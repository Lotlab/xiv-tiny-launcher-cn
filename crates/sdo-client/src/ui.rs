//! 层 2 与 EXE 的交互口：轮询循环在层 2，EXE 通过 [`Ui`] 展示进度、干预流程。
//! 二维码落盘与渲染、按键、终端输出都在 EXE 侧。

/// 层 2 需要 EXE 提供的能力。
pub trait Ui {
    /// 拿到新二维码（第 `round` 张）。EXE 负责落盘与渲染。
    fn show_code(&mut self, png: &[u8], round: u32);

    /// 每轮轮询前问一次。
    fn tick(&mut self, wait: Wait) -> Action;

    /// 当前的「保持登录」勾选。
    ///
    /// EXE 自己维护这个状态（按 k 时直接改），层 2 每轮来读 —— 它是**请求参数**
    /// （`keepLoginFlag`），必须与界面上显示的一致。
    fn keep_login(&self) -> bool;

    /// 一次性事件。
    fn note(&mut self, note: Note);
}

/// 轮询进度。
#[derive(Debug, Clone, Copy)]
pub struct Wait {
    /// 本张码 / 本次确认的剩余秒数。
    pub left_secs: u64,
    pub attempt: u32,
    pub phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    ScanCode,
    ConfirmPush,
}

/// EXE 对流程的干预。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wait,
    /// 作废本张码（或本次手机确认），换一张 / 重发一次。
    RefreshCode,
    Quit,
}

/// 一次性事件；带文本的都是展示用原文。
#[derive(Debug, Clone)]
pub enum Note {
    Scanned,
    CodeRefreshed(&'static str),
    PushSent,
    ServerError(String),
    ExchangeFailed(String),
}
