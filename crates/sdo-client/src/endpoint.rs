//! 接口端点：每个服务端 API 是一个结构体，query 拼接只发生在其 `path()` 内部。
//!
//! 模板与线上 `0.0.0.26` 的 `SdoBaseClient.dll` 逐字核对过。注意本地
//! `Launcher3Modules` 那份是 `0.0.0.18`（无 `groupId`/`channelId`），别拿它当基准。
//!
//! - [`Endpoint`]：`HOST` + `TIMEOUT` + `path()`；[`Api`](crate::Api) 只接受端点。
//! - [`Suffix`]：公共后缀（字段顺序即拼装顺序），各端点持有它拼出完整 `path?query`。

use crate::consts::*;
use proto::enc;

use crate::transport::{Identity, Timeout};

/// 服务端接口端点。
pub trait Endpoint {
    /// 固定 host（实测值，不自动切换）。
    const HOST: &'static str;
    /// 超时种类（CAS 认证短超时 / 下载类长超时）。
    const TIMEOUT: Timeout;
    /// `path?query`（host 由 [`Api`](crate::Api) 按 `HOST` 指定）。
    fn path(&self) -> String;
}

/// 一个 App 的冻结口径：身份 + 它自己的一个版本号。
///
/// 三个 id 是整数（官方模板里是 `%d`）；没有 `app_site` —— 逆向确认 `appIdSite`
/// 与 `appId` 取同一处。换票是「从当前 App 换到新的 App」，两步各取对应的版本号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct App {
    pub app_id: i32,
    pub group: i32,
    /// 登录系模板的 `areaId`（固定 `1`）；游戏应用是本次选区。
    pub area_id: i32,
    /// `scene`；登录系模板不带（`None`）。
    pub scene: Option<&'static str>,
    /// 这个 App 自己的版本号。
    pub product_version: &'static str,
}

/// 登录应用（QR/push/fast/附属请求）。版本出处：`Launcher3Modules/sdologin/version.txt`。
pub const LOGIN_APP: App = App {
    app_id: 791000814,
    group: 1,
    area_id: 1,
    scene: None,
    product_version: "1.1.344.45",
};

/// 游戏应用的 `appId`：区服表路径与 `-AppID=` 用它（那时选区还没定）。
pub const GAME_APP_ID: i32 = 100001900;

impl App {
    /// 游戏应用（SSO 换票）+ 本次选区。版本出处：`sdo/sdologin/version.txt` 的 Base version。
    pub fn game(area_id: i32) -> App {
        App {
            app_id: GAME_APP_ID,
            group: -1,
            area_id,
            scene: Some("V3Launcher"),
            product_version: "1.9.7.18",
        }
    }
}

/// 公共后缀。字段顺序即拼装顺序。
///
/// 省略约定不统一是跟服务端模板走的：`groupId/scene` 用 `Option`（整段无此参数）；
/// `channelId/productVersion` 用空串（保留参数名、值为空，`ssoAuthorizationLogin` 要求）；
/// `tag` 用 `-1` 哨兵（`-1` 时跳过拼装，它是协议值而不只是“无”）。
#[derive(Debug, Clone)]
pub struct Suffix {
    pub app_id: String,
    pub area_id: String,
    /// `None` = 该模板不含 `groupId`（fastInLogin / getSystemConfig）。
    pub group_id: Option<String>,
    pub device_id: String,
    pub mac_id: String,
    pub ep_ip: String,
    pub ep_name_enc: String,
    /// 插在 `epName` 之后（SSO 的 `scene=V3Launcher`）。
    pub scene: Option<String>,
    pub run_time_id: String,
    /// 空串 = 保留参数名、值为空（`ssoAuthorizationLogin`）。
    pub channel_id: String,
    /// CAS 链：已做 URL 编码（`.`→`%2E`）；空串 = 该模板不带 `productVersion`。
    pub product_version: String,
    /// 服务端 `tag`；`-1` = 该模板不带 `tag`。
    pub tag: i32,
}

impl Suffix {
    /// 登录应用后缀（QR/push/promotion/loginUserInfo）。
    ///
    /// 模板形状由构造函数固定：登录系模板不含 `scene`，即使 `app` 带了也不拼。
    pub fn login(id: &Identity, run_time_id: &str, app: &App) -> Suffix {
        Suffix {
            app_id: app.app_id.to_string(),
            area_id: app.area_id.to_string(),
            group_id: Some(app.group.to_string()),
            device_id: id.device_id.clone(),
            mac_id: id.mac_id.clone(),
            ep_ip: id.ep_ip.clone(),
            ep_name_enc: enc::url_encode(&id.ep_name),
            scene: None,
            run_time_id: run_time_id.into(),
            channel_id: CHANNEL_ID.into(),
            product_version: enc::url_encode(app.product_version),
            tag: TAG,
        }
    }

    /// 登录应用后缀但**不含 `groupId`**（`fastInLogin` / `getSystemConfig`）。
    pub fn login_no_group(id: &Identity, run_time_id: &str, app: &App) -> Suffix {
        let mut s = Suffix::login(id, run_time_id, app);
        s.group_id = None;
        s
    }

    /// `getSsoAuthorization` 后缀（换票第一步）。
    ///
    /// 身份（`appId/areaId/groupId/scene`）取**换入的** App（`to`），
    /// `productVersion` 取**发起换票的当前** App（`from`）：以当前应用的版本
    /// 申请进入目标应用的授权。
    pub fn for_sso_authorization(
        id: &Identity,
        run_time_id: &str,
        from: &App,
        to: &App,
    ) -> Suffix {
        Suffix {
            app_id: to.app_id.to_string(),
            area_id: to.area_id.to_string(),
            group_id: Some(to.group.to_string()),
            device_id: id.device_id.clone(),
            mac_id: id.mac_id.clone(),
            ep_ip: id.ep_ip.clone(),
            ep_name_enc: enc::url_encode(&id.ep_name),
            scene: to.scene.map(str::to_string),
            run_time_id: run_time_id.into(),
            channel_id: CHANNEL_ID.into(),
            product_version: enc::url_encode(from.product_version),
            tag: TAG,
        }
    }

    /// `ssoAuthorizationLogin` 后缀（换票第二步）：无 `guid/tgt`，`epIp/runTimeId/channelId` 值为空。
    ///
    /// 身份与 `productVersion` 都取换入的 App（`to`）—— 此时已经“就是”那个 App。
    ///
    /// 这一步在官方由**游戏侧** sdologin 发出，它的 `channelId` 键是 `[ChannelId] value`（不是
    /// 启动器侧的 `[Skin] value`），缺键即空串 —— 官方包里没有该键，空值就是官方默认。
    pub fn for_sso_login(id: &Identity, run_time_id: &str, to: &App) -> Suffix {
        let mut s = Suffix::for_sso_authorization(id, run_time_id, to, to);
        s.ep_ip = String::new();
        s.run_time_id = String::new();
        s.channel_id = String::new();
        s
    }

    /// 拼装公共后缀（不含前导 `&`）。
    pub fn to_query(&self) -> String {
        let mut q = String::with_capacity(420);
        q.push_str("authenSource=");
        q.push_str(AUTHEN_SOURCE);
        q.push_str("&appId=");
        q.push_str(&self.app_id);
        q.push_str("&areaId=");
        q.push_str(&self.area_id);
        if let Some(g) = &self.group_id {
            q.push_str("&groupId=");
            q.push_str(g);
        }
        // `appIdSite` 与 `appId` 同值（见 `App` 的说明）。
        q.push_str("&appIdSite=");
        q.push_str(&self.app_id);
        q.push_str("&locale=");
        q.push_str(LOCALE);
        q.push_str("&productId=");
        q.push_str(&PRODUCT_ID.to_string());
        q.push_str("&frameType=");
        q.push_str(FRAME_TYPE);
        q.push_str("&endpointOS=");
        q.push_str(ENDPOINT_OS);
        q.push_str("&version=");
        q.push_str(VERSION);
        q.push_str("&customSecurityLevel=");
        q.push_str(&CUSTOM_SECURITY_LEVEL.to_string());
        q.push_str("&deviceId=");
        q.push_str(&self.device_id);
        q.push_str("&thirdLoginExtern=");
        q.push_str(THIRD_LOGIN_EXTERN);
        q.push_str("&macId=");
        q.push_str(&self.mac_id);
        q.push_str("&epIp=");
        q.push_str(&self.ep_ip);
        q.push_str("&epName=");
        q.push_str(&self.ep_name_enc);
        if let Some(scene) = &self.scene {
            q.push_str("&scene=");
            q.push_str(scene);
        }
        q.push_str("&extendInfo=&sdoVersion=&runTimeId=");
        q.push_str(&self.run_time_id);
        q.push_str("&channelId=");
        q.push_str(&self.channel_id);
        if !self.product_version.is_empty() {
            q.push_str("&productVersion=");
            q.push_str(&self.product_version);
        }
        if self.tag != -1 {
            q.push_str("&tag=");
            q.push_str(&self.tag.to_string());
        }
        q
    }
}

/// `getGuid.json`（`guid` 记下保留到 SSO；`dynamicKey` 备用）。
pub struct GetGuid {
    suffix: Suffix,
}

impl GetGuid {
    pub fn new(suffix: Suffix) -> GetGuid {
        GetGuid { suffix }
    }
}

impl Endpoint for GetGuid {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!("/authen/getGuid.json?generateDynamicKey=1&{}", self.suffix.to_query())
    }
}

/// `getCodeKey.json`（体为 PNG，`codeKey` 取自响应头 `CODEKEY`）。
pub struct GetCodeKey {
    suffix: Suffix,
}

impl GetCodeKey {
    pub fn new(suffix: Suffix) -> GetCodeKey {
        GetCodeKey { suffix }
    }
}

impl Endpoint for GetCodeKey {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!("/authen/getCodeKey.json?maxsize=128&{}", self.suffix.to_query())
    }
}

/// `codeKeyLogin.json`（二维码轮询）。
pub struct CodeKeyLogin {
    suffix: Suffix,
    code_key: String,
    guid: String,
    keep_flag: i32,
}

impl CodeKeyLogin {
    pub fn new(suffix: Suffix, code_key: impl Into<String>, guid: impl Into<String>, keep_flag: i32) -> CodeKeyLogin {
        CodeKeyLogin {
            suffix,
            code_key: code_key.into(),
            guid: guid.into(),
            keep_flag,
        }
    }
}

impl Endpoint for CodeKeyLogin {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/codeKeyLogin.json?codeKey={}&guid={}&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag={}&maxsize=128&{}",
            self.code_key,
            self.guid,
            self.keep_flag,
            self.suffix.to_query()
        )
    }
}

/// `fastInLogin`（无 `groupId`）。
pub struct FastInLogin {
    suffix: Suffix,
    keep_key: String,
}

impl FastInLogin {
    pub fn new(suffix: Suffix, keep_key: impl Into<String>) -> FastInLogin {
        FastInLogin {
            suffix,
            keep_key: keep_key.into(),
        }
    }
}

impl Endpoint for FastInLogin {
    const HOST: &'static str = HOST_N_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/v2/fastInLogin?keepLoginKey={}&{}",
            self.keep_key,
            self.suffix.to_query()
        )
    }
}

/// `cancelPushMessageLogin.json`（空 key 必回 `-10242301`，忽略）。
pub struct CancelPush {
    suffix: Suffix,
    guid: String,
}

impl CancelPush {
    pub fn new(suffix: Suffix, guid: impl Into<String>) -> CancelPush {
        CancelPush {
            suffix,
            guid: guid.into(),
        }
    }
}

impl Endpoint for CancelPush {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/cancelPushMessageLogin.json?pushMsgSessionKey=&guid={}&{}",
            self.guid,
            self.suffix.to_query()
        )
    }
}

/// `sendPushMessage.json`（`inputUserId` 按公共后缀编码规则编码；官方不带 `guid`）。
pub struct SendPush {
    suffix: Suffix,
    account: String,
}

impl SendPush {
    pub fn new(suffix: Suffix, account: impl Into<String>) -> SendPush {
        SendPush {
            suffix,
            account: account.into(),
        }
    }
}

impl Endpoint for SendPush {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/sendPushMessage.json?inputUserId={}&scene=pc_pushmsglogin&{}",
            enc::url_encode(&self.account),
            self.suffix.to_query()
        )
    }
}

/// `pushMessageLogin.json`（无 `maxsize`）。
pub struct PushLogin {
    suffix: Suffix,
    session_key: String,
    guid: String,
}

impl PushLogin {
    pub fn new(suffix: Suffix, session_key: impl Into<String>, guid: impl Into<String>) -> PushLogin {
        PushLogin {
            suffix,
            session_key: session_key.into(),
            guid: guid.into(),
        }
    }
}

impl Endpoint for PushLogin {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/pushMessageLogin.json?pushMsgSessionKey={}&guid={}&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&{}",
            self.session_key,
            self.guid,
            self.suffix.to_query()
        )
    }
}

/// `getSsoAuthorization`（用 `tgt0/guid0` 换 `authorization`）。
pub struct SsoAuthorization {
    suffix: Suffix,
    tgt: String,
    guid: String,
}

impl SsoAuthorization {
    pub fn new(suffix: Suffix, tgt: impl Into<String>, guid: impl Into<String>) -> SsoAuthorization {
        SsoAuthorization {
            suffix,
            tgt: tgt.into(),
            guid: guid.into(),
        }
    }
}

impl Endpoint for SsoAuthorization {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/getSsoAuthorization?tgt={}&guid={}&{}",
            self.tgt,
            self.guid,
            self.suffix.to_query()
        )
    }
}

/// `ssoAuthorizationLogin`（用 `authorization` 换 `ticket1`；无 `guid/tgt`）。
pub struct SsoLogin {
    suffix: Suffix,
    authorization: String,
}

impl SsoLogin {
    pub fn new(suffix: Suffix, authorization: impl Into<String>) -> SsoLogin {
        SsoLogin {
            suffix,
            authorization: authorization.into(),
        }
    }
}

impl Endpoint for SsoLogin {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/authen/ssoAuthorizationLogin?authorization={}&{}",
            self.authorization,
            self.suffix.to_query()
        )
    }
}

/// 区服表（`time` 为 13 位毫秒缓存失效参数；`app_id` 取游戏应用）。
pub struct ServerJson {
    app_id: String,
    millis: u128,
}

impl ServerJson {
    pub fn new(app_id: i32, millis: u128) -> ServerJson {
        ServerJson {
            app_id: app_id.to_string(),
            millis,
        }
    }
}

impl Endpoint for ServerJson {
    const HOST: &'static str = HOST_V3LAUNCHER;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!(
            "/v3launcher/server/{}/8847/server.json?time={}",
            self.app_id, self.millis
        )
    }
}

/// 用户协议（可选；`appid` 取登录应用）。
pub struct Agreement {
    app_id: String,
}

impl Agreement {
    pub fn new(app_id: impl std::fmt::Display) -> Agreement {
        Agreement {
            app_id: app_id.to_string(),
        }
    }
}

impl Endpoint for Agreement {
    const HOST: &'static str = HOST_UTILITY;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!(
            "/agreement/user?appid={}&scene=optimisepc&privacypolicyversion=3&serviceAgreementVersion=7",
            self.app_id
        )
    }
}

/// 人脸验证初始化（`bizVersion` 点号原样；顶层字段是 `resultCode` 而非 `return_code`）。
/// `appId/areaId/bizVersion` 取登录应用口径。
pub struct FaceVerifyInit {
    device_id: String,
    app_id: String,
    area_id: String,
    product_version: String,
    tgt: String,
}

impl FaceVerifyInit {
    pub fn new(
        device_id: impl std::fmt::Display,
        app_id: impl std::fmt::Display,
        area_id: impl std::fmt::Display,
        product_version: impl std::fmt::Display,
        tgt: impl std::fmt::Display,
    ) -> FaceVerifyInit {
        FaceVerifyInit {
            device_id: device_id.to_string(),
            app_id: app_id.to_string(),
            area_id: area_id.to_string(),
            product_version: product_version.to_string(),
            tgt: tgt.to_string(),
        }
    }
}

impl Endpoint for FaceVerifyInit {
    const HOST: &'static str = HOST_GFC;
    const TIMEOUT: Timeout = Timeout::Auth;
    fn path(&self) -> String {
        format!(
            "/api/faceVerify/init?authenType=1&appId={}&scene=face_login&deviceId={}&authenToken={}&areaId={}&bizVersion={}",
            self.app_id, self.device_id, self.tgt, self.area_id, self.product_version
        )
    }
}

/// `getPromotionInfo.json`。
pub struct Promotion {
    suffix: Suffix,
    tgt: String,
}

impl Promotion {
    pub fn new(suffix: Suffix, tgt: impl Into<String>) -> Promotion {
        Promotion {
            suffix,
            tgt: tgt.into(),
        }
    }
}

impl Endpoint for Promotion {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!(
            "/authen/getPromotionInfo.json?tgt={}&promotionFlag=1&{}",
            self.tgt,
            self.suffix.to_query()
        )
    }
}

/// `getLoginUserInfo.json`。
pub struct LoginUserInfo {
    suffix: Suffix,
    tgt: String,
}

impl LoginUserInfo {
    pub fn new(suffix: Suffix, tgt: impl Into<String>) -> LoginUserInfo {
        LoginUserInfo {
            suffix,
            tgt: tgt.into(),
        }
    }
}

impl Endpoint for LoginUserInfo {
    const HOST: &'static str = HOST_CAS;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!(
            "/authen/getLoginUserInfo.json?tgt={}&{}",
            self.tgt,
            self.suffix.to_query()
        )
    }
}

/// `getSystemConfig`（无 `groupId`；只记日志不分支）。
pub struct SystemConfig {
    suffix: Suffix,
}

impl SystemConfig {
    pub fn new(suffix: Suffix) -> SystemConfig {
        SystemConfig { suffix }
    }
}

impl Endpoint for SystemConfig {
    const HOST: &'static str = HOST_N_CAS;
    const TIMEOUT: Timeout = Timeout::Download;
    fn path(&self) -> String {
        format!(
            "/authen/v2/getSystemConfig?logintype=godown&{}",
            self.suffix.to_query()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Identity {
        Identity {
            device_id: "88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:".into(),
            mac_id: "00-11-22-33-44-55".into(),
            ep_ip: "192.168.1.100".into(),
            ep_name: "DESKTOP-ABC1234".into(),
        }
    }

    fn login_app() -> App {
        LOGIN_APP
    }

    fn game_app() -> App {
        App::game(7)
    }

    const RTID: &str = "6EC5EF3932F14524AEC4862361942649";

    /// 公共后缀按固定顺序逐字拼装。
    #[test]
    fn login_suffix_param_order() {
        let s = Suffix::login(&id(), RTID, &login_app());
        let q = s.to_query();
        let expected = "authenSource=1&appId=791000814&areaId=1&groupId=1&appIdSite=791000814\
&locale=zh_CN&productId=4&frameType=1&endpointOS=1&version=21&customSecurityLevel=2\
&deviceId=88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:\
&thirdLoginExtern=0&macId=00-11-22-33-44-55&epIp=192.168.1.100&epName=DESKTOP%2DABC1234\
&extendInfo=&sdoVersion=&runTimeId=6EC5EF3932F14524AEC4862361942649&channelId=0\
&productVersion=1%2E1%2E344%2E45&tag=0";
        assert_eq!(q, expected);
    }

    #[test]
    fn no_group_suffix_drops_group_id() {
        let s = Suffix::login_no_group(&id(), RTID, &login_app());
        let q = s.to_query();
        assert!(!q.contains("groupId="), "{q}");
        assert!(
            q.contains("&appId=791000814&areaId=1&appIdSite=791000814&"),
            "{q}"
        );
    }

    #[test]
    fn code_key_login_prefix_order() {
        let s = Suffix::login(&id(), RTID, &login_app());
        let p = CodeKeyLogin::new(
            s,
            "0123456789abcdef0123456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF",
            1,
        )
        .path();
        assert!(
            p.starts_with(
                "/authen/codeKeyLogin.json?codeKey=0123456789abcdef0123456789abcdef\
&guid=0123456789ABCDEF0123456789ABCDEF\
&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&maxsize=128&authenSource=1&"
            ),
            "{p}"
        );
        assert_eq!(CodeKeyLogin::HOST, HOST_CAS);
        assert_eq!(CodeKeyLogin::TIMEOUT, Timeout::Auth);
    }

    #[test]
    fn get_guid_and_code_key_prefixes() {
        let s = Suffix::login(&id(), RTID, &login_app());
        assert!(GetGuid::new(s.clone()).path().starts_with("/authen/getGuid.json?generateDynamicKey=1&authenSource=1&appId=791000814&areaId=1&groupId=1&appIdSite=791000814&"));
        assert!(GetCodeKey::new(s).path()
            .starts_with("/authen/getCodeKey.json?maxsize=128&authenSource=1&"));
    }

    #[test]
    fn fast_in_login_template() {
        let s = Suffix::login_no_group(&id(), RTID, &login_app());
        let p = FastInLogin::new(s, "ULSKLK-TEST").path();
        assert!(p.starts_with(
            "/authen/v2/fastInLogin?keepLoginKey=ULSKLK-TEST&authenSource=1&appId=791000814&areaId=1&appIdSite=791000814&"
        ), "{p}");
        assert!(!p.contains("groupId"), "{p}");
        assert_eq!(FastInLogin::HOST, HOST_N_CAS);
    }

    #[test]
    fn push_templates() {
        let s = Suffix::login(&id(), RTID, &login_app());
        assert!(CancelPush::new(s.clone(), "G1").path().starts_with(
            "/authen/cancelPushMessageLogin.json?pushMsgSessionKey=&guid=G1&authenSource=1&"
        ));
        let send = SendPush::new(s.clone(), "user@example.com").path();
        assert!(send.starts_with(
            "/authen/sendPushMessage.json?inputUserId=user%40example%2Ecom&scene=pc_pushmsglogin&authenSource=1&"
        ), "{send}");
        assert!(!send.contains("&guid="), "官方构造器不带 guid: {send}");
        let poll = PushLogin::new(s, "S1", "G1").path();
        assert!(poll.starts_with(
            "/authen/pushMessageLogin.json?pushMsgSessionKey=S1&guid=G1&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&authenSource=1&"
        ));
        assert!(!poll.contains("maxsize"), "push 轮询不得带 maxsize");
    }

    #[test]
    fn sso_legs_match_template() {
        let s1 = Suffix::for_sso_authorization(&id(), RTID, &login_app(), &game_app());
        let p1 = SsoAuthorization::new(s1, "ULSTGT-T0", "GUID0").path();
        let expected1 = "authenSource=1&appId=100001900&areaId=7&groupId=-1&appIdSite=100001900\
&locale=zh_CN&productId=4&frameType=1&endpointOS=1&version=21&customSecurityLevel=2\
&deviceId=88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:\
&thirdLoginExtern=0&macId=00-11-22-33-44-55&epIp=192.168.1.100&epName=DESKTOP%2DABC1234\
&scene=V3Launcher&extendInfo=&sdoVersion=&runTimeId=6EC5EF3932F14524AEC4862361942649&channelId=0\
&productVersion=1%2E1%2E344%2E45&tag=0";
        assert_eq!(
            p1,
            format!("/authen/getSsoAuthorization?tgt=ULSTGT-T0&guid=GUID0&{expected1}")
        );

        let s2 = Suffix::for_sso_login(&id(), RTID, &game_app());
        let p2 = SsoLogin::new(s2, "UA-123").path();
        // `ssoAuthorizationLogin`：无 guid/tgt；epIp/runTimeId/channelId 值为空；版本 1.9.7.18
        assert!(
            p2.starts_with("/authen/ssoAuthorizationLogin?authorization=UA-123&authenSource=1&")
        );
        assert!(!p2.contains("guid="), "{p2}");
        assert!(!p2.contains("tgt="), "{p2}");
        assert!(
            p2.contains("&epIp=&epName=DESKTOP%2DABC1234&scene=V3Launcher&"),
            "{p2}"
        );
        assert!(
            p2.contains("&runTimeId=&channelId=&productVersion=1%2E9%2E7%2E18&tag=0"),
            "{p2}"
        );
        assert!(
            p2.contains(
                "&deviceId=88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:"
            ),
            "{p2}"
        );
        assert!(p2.contains("&macId=00-11-22-33-44-55&"), "{p2}");
    }

    #[test]
    fn aux_templates_keep_raw_dots() {
        let s = Suffix::login(&id(), RTID, &login_app());
        let fv = FaceVerifyInit::new(
            "88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:",
            LOGIN_APP.app_id,
            LOGIN_APP.area_id,
            LOGIN_APP.product_version,
            "ULSTGT-T0",
        )
        .path();
        assert_eq!(
            fv,
            "/api/faceVerify/init?authenType=1&appId=791000814&scene=face_login\
&deviceId=88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:\
&authenToken=ULSTGT-T0&areaId=1&bizVersion=1.1.344.45"
        );
        assert!(Promotion::new(s.clone(), "T").path()
            .starts_with("/authen/getPromotionInfo.json?tgt=T&promotionFlag=1&authenSource=1&"));
        assert!(LoginUserInfo::new(s.clone(), "T").path()
            .starts_with("/authen/getLoginUserInfo.json?tgt=T&authenSource=1&"));
        let cfg = SystemConfig::new(Suffix::login_no_group(&id(), RTID, &login_app())).path();
        assert!(cfg.starts_with("/authen/v2/getSystemConfig?logintype=godown&authenSource=1&appId=791000814&areaId=1&appIdSite=791000814&"), "{cfg}");
        assert!(!cfg.contains("groupId"), "{cfg}");
        assert_eq!(Agreement::new(LOGIN_APP.app_id).path(), "/agreement/user?appid=791000814&scene=optimisepc&privacypolicyversion=3&serviceAgreementVersion=7");
        assert_eq!(ServerJson::HOST, HOST_V3LAUNCHER);
    }

    #[test]
    fn tag_minus_one_skips_param() {
        let mut s = Suffix::login(&id(), RTID, &login_app());
        s.tag = -1;
        assert!(!s.to_query().contains("&tag="));
        // channelId 非零：只改值，不改模板形状
        let mut s2 = Suffix::login(&id(), RTID, &login_app());
        s2.channel_id = "3".into();
        assert!(s2.to_query().contains("&channelId=3&"));
    }

    #[test]
    fn server_json_path_shape() {
        assert_eq!(
            ServerJson::new(GAME_APP_ID, 1759400000123).path(),
            "/v3launcher/server/100001900/8847/server.json?time=1759400000123"
        );
    }
}
