//! M3 端到端：合成 patch zip（含真实 xdelta3 生成的 delta）→ 解出 → 原地应用 → 校验。
//!
//! delta 由真实 `xdelta3 -e -f -s source target` 生成后作为 fixture 提交（见
//! `tests/fixtures/`），所以不依赖系统里有没有 `xdelta3`，测试始终会跑。

use std::io::Write;

use patcher::delta::{self, DeltaOutcome};
use patcher::hash::md5_hex_upper;
use patcher::patch;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{FIXTURES}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"))
}

/// 用 fixture 的 source/target/delta 合成一个补丁 zip。
fn build_patch_zip(dir: &std::path::Path, delta_bytes: &[u8]) -> std::path::PathBuf {
    let source = fixture("source.bin");
    let target = fixture("target.bin");
    let xml = format!(
        r#"<XMLROOT>
<DeltaPathInfo><DeltaPathSubItem Key="game\file.dat" Value="Pkg\game\file.dat.delta"/></DeltaPathInfo>
<DeltaMD5Info><DeltaMD5SubItem Key="game\file.dat.delta" Value="{}"/></DeltaMD5Info>
<OriginMD5Info><OriginMD5SubItem Key="game\file.dat" Value="{}"/></OriginMD5Info>
<ResultMD5Info><ResultMD5SubItem Key="game\file.dat" Value="{}"/></ResultMD5Info>
</XMLROOT>"#,
        md5_hex_upper(delta_bytes),
        md5_hex_upper(&source),
        md5_hex_upper(&target),
    );

    let zip_path = dir.join("patch.zip");
    let f = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("patch_delta_direct.dat", opts).unwrap();
    zw.write_all(xml.as_bytes()).unwrap();
    zw.start_file(r"Pkg\game\file.dat.delta", opts).unwrap();
    zw.write_all(delta_bytes).unwrap();
    zw.finish().unwrap();
    zip_path
}

#[test]
fn extract_and_apply_delta_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let delta_bytes = fixture("file.dat.delta");
    let source = fixture("source.bin");
    let target = fixture("target.bin");

    // 解出 delta。
    let zip_path = build_patch_zip(root, &delta_bytes);
    let delta_dir = root.join("deltas");
    let entries = patch::extract_deltas(&[zip_path], &delta_dir).unwrap();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.rel_path, "file.dat");
    assert_eq!(e.delta_rel(), "Pkg/game/file.dat.delta");
    assert!(delta_dir.join(e.delta_rel()).is_file(), "delta 未解出");

    // 在原地把 delta 打到源文件上。
    let game_dir = root.join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    let game_file = game_dir.join("file.dat");
    std::fs::write(&game_file, &source).unwrap();

    let outcome = delta::apply_in_place(
        &game_file,
        &delta_dir.join(e.delta_rel()),
        &e.delta_md5,
        &e.origin_md5,
        &e.result_md5,
    )
    .unwrap();
    assert_eq!(outcome, DeltaOutcome::Applied);
    assert_eq!(std::fs::read(&game_file).unwrap(), target, "结果与目标不一致");

    // 幂等：再打一次应跳过（源已是目标内容）。
    let again = delta::apply_in_place(
        &game_file,
        &delta_dir.join(e.delta_rel()),
        &e.delta_md5,
        &e.origin_md5,
        &e.result_md5,
    )
    .unwrap();
    assert_eq!(again, DeltaOutcome::Skipped);
}

#[test]
fn apply_rejects_wrong_origin_md5() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let delta_bytes = fixture("file.dat.delta");
    let source = fixture("source.bin");
    let target = fixture("target.bin");

    let zip_path = build_patch_zip(root, &delta_bytes);
    let delta_dir = root.join("deltas");
    let entries = patch::extract_deltas(&[zip_path], &delta_dir).unwrap();
    let e = &entries[0];

    // 源文件被改坏 → origin_md5 不符 → 报错。
    let game_file = root.join("game.dat");
    std::fs::write(&game_file, b"corrupted content!").unwrap();
    let err = delta::apply_in_place(
        &game_file,
        &delta_dir.join(e.delta_rel()),
        &e.delta_md5,
        &md5_hex_upper(&source),
        &md5_hex_upper(&target),
    );
    assert!(err.is_err(), "源 MD5 不符时应报错");
}

/// delta 自身与清单不符（下坏 / 被改）时必须在动源文件之前就拦住。
#[test]
fn apply_rejects_delta_that_fails_manifest_md5() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let delta_bytes = fixture("file.dat.delta");
    let source = fixture("source.bin");
    let target = fixture("target.bin");

    let zip_path = build_patch_zip(root, &delta_bytes);
    let delta_dir = root.join("deltas");
    let entries = patch::extract_deltas(&[zip_path], &delta_dir).unwrap();
    let e = &entries[0];

    // 把解出的 delta 改坏，清单里的 delta_md5 就不符了。
    let delta_file = delta_dir.join(e.delta_rel());
    std::fs::write(&delta_file, b"corrupted delta").unwrap();

    let game_dir = root.join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    let game_file = game_dir.join("file.dat");
    std::fs::write(&game_file, &source).unwrap();

    let err = delta::apply_in_place(
        &game_file,
        &delta_file,
        &e.delta_md5,
        &md5_hex_upper(&source),
        &md5_hex_upper(&target),
    );
    assert!(matches!(err, Err(patcher::delta::DeltaError::BadDelta(_))), "{err:?}");
    assert_eq!(std::fs::read(&game_file).unwrap(), source, "源文件不该被改");
}
