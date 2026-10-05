//! DLL 侧的字面值：日志名与 Login 系列 IID。

pub const LOG_DLL: &str = "sdologinentry.log";

/// Login 系列 IID：游戏实际传第二个值，其余为表值兼容。
pub const IID_LOGIN: [&str; 4] = [
    "AD887932-2D1C-48EC-B30E-535B609C12D6",
    "7B06DAD6-6832-4455-AFC6-6C8BE902534B",
    "D09EE9A2-8C44-42D6-9F3E-E34727BBEA10",
    "D09EE9A2-8C44-42D6-9FE2-E34727BBEA10",
];
pub const IID_INFO: &str = "2B6523B0-9D08-424B-94CC-6BA173C2DF26";
