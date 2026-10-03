//! URL 模板与参数顺序：字段顺序即拼装顺序，全部字面固定。
//!
//! 本模块只拼 `path?query`，host 由 launcher 侧按 `consts::HOST_*` 指定。

use crate::consts::*;
use crate::device::Device;
use crate::enc;

/// 公共后缀。字段顺序即拼装顺序。
#[derive(Debug, Clone)]
pub struct Suffix {
    pub app_id: String,
    pub area_id: String,
    /// `None` = 该模板不含 `groupId`（fastInLogin / getSystemConfig）。
    pub group_id: Option<String>,
    pub app_id_site: String,
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
    /// 登录应用后缀（QR/push/promotion/loginUserInfo）：`appId=791000814 areaId=1 groupId=1`。
    pub fn login(device: &Device, run_time_id: &str) -> Suffix {
        Suffix {
            app_id: LOGIN_APP_ID.into(),
            area_id: LOGIN_AREA_ID.into(),
            group_id: Some(LOGIN_GROUP_ID.into()),
            app_id_site: LOGIN_APP_ID_SITE.into(),
            device_id: device.device_id.clone(),
            mac_id: device.mac_id.clone(),
            ep_ip: device.ep_ip.clone(),
            ep_name_enc: device.ep_name_encoded(),
            scene: None,
            run_time_id: run_time_id.into(),
            channel_id: CHANNEL_ID.into(),
            product_version: enc::url_encode(LOGIN_PRODUCT_VERSION),
            tag: TAG,
        }
    }

    /// 登录应用后缀但**不含 `groupId`**（`fastInLogin` / `getSystemConfig`）。
    pub fn login_no_group(device: &Device, run_time_id: &str) -> Suffix {
        let mut s = Suffix::login(device, run_time_id);
        s.group_id = None;
        s
    }

    /// `getSsoAuthorization` 后缀（`appId=100001900 areaId=<选区> groupId=-1 scene=V3Launcher`）。
    pub fn for_sso_authorization(device: &Device, run_time_id: &str, area_id: &str) -> Suffix {
        Suffix {
            app_id: GAME_APP_ID.into(),
            area_id: area_id.into(),
            group_id: Some(GAME_GROUP_ID.into()),
            app_id_site: GAME_APP_ID_SITE.into(),
            device_id: device.device_id.clone(),
            mac_id: device.mac_id.clone(),
            ep_ip: device.ep_ip.clone(),
            ep_name_enc: device.ep_name_encoded(),
            scene: Some(SSO_SCENE.into()),
            run_time_id: run_time_id.into(),
            channel_id: CHANNEL_ID.into(),
            product_version: enc::url_encode(SSO_AUTHORIZATION_PRODUCT_VERSION),
            tag: TAG,
        }
    }

    /// `ssoAuthorizationLogin` 后缀：无 `guid/tgt`，`epIp/runTimeId/channelId` 值为空，版本号为 `1.9.7.10`。
    pub fn for_sso_login(device: &Device, run_time_id: &str, area_id: &str) -> Suffix {
        let mut s = Suffix::for_sso_authorization(device, run_time_id, area_id);
        s.ep_ip = String::new();
        s.run_time_id = String::new();
        s.channel_id = String::new();
        s.product_version = enc::url_encode(SSO_LOGIN_PRODUCT_VERSION);
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
        q.push_str("&appIdSite=");
        q.push_str(&self.app_id_site);
        q.push_str("&locale=");
        q.push_str(LOCALE);
        q.push_str("&productId=");
        q.push_str(PRODUCT_ID);
        q.push_str("&frameType=");
        q.push_str(FRAME_TYPE);
        q.push_str("&endpointOS=");
        q.push_str(ENDPOINT_OS);
        q.push_str("&version=");
        q.push_str(VERSION);
        q.push_str("&customSecurityLevel=");
        q.push_str(CUSTOM_SECURITY_LEVEL);
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

pub fn path_get_guid(s: &Suffix) -> String {
    format!("/authen/getGuid.json?generateDynamicKey=1&{}", s.to_query())
}

pub fn path_get_code_key(s: &Suffix) -> String {
    format!("/authen/getCodeKey.json?maxsize=128&{}", s.to_query())
}

pub fn path_code_key_login(s: &Suffix, code_key: &str, guid: &str, keep_login_flag: i32) -> String {
    format!(
        "/authen/codeKeyLogin.json?codeKey={}&guid={}&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag={}&maxsize=128&{}",
        code_key,
        guid,
        keep_login_flag,
        s.to_query()
    )
}

pub fn path_fast_in_login(s: &Suffix, keep_login_key: &str) -> String {
    format!(
        "/authen/v2/fastInLogin?keepLoginKey={}&{}",
        keep_login_key,
        s.to_query()
    )
}

pub fn path_cancel_push_message_login(s: &Suffix, guid: &str) -> String {
    format!(
        "/authen/cancelPushMessageLogin.json?pushMsgSessionKey=&guid={}&{}",
        guid,
        s.to_query()
    )
}

pub fn path_send_push_message(s: &Suffix, account_raw: &str, guid: &str) -> String {
    format!(
        "/authen/sendPushMessage.json?inputUserId={}&scene=pc_pushmsglogin&guid={}&{}",
        enc::url_encode(account_raw),
        guid,
        s.to_query()
    )
}

pub fn path_push_message_login(s: &Suffix, session_key: &str, guid: &str) -> String {
    format!(
        "/authen/pushMessageLogin.json?pushMsgSessionKey={}&guid={}&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&{}",
        session_key,
        guid,
        s.to_query()
    )
}

pub fn path_get_sso_authorization(s: &Suffix, tgt0: &str, guid0: &str) -> String {
    format!(
        "/authen/getSsoAuthorization?tgt={}&guid={}&{}",
        tgt0,
        guid0,
        s.to_query()
    )
}

pub fn path_sso_authorization_login(s: &Suffix, authorization: &str) -> String {
    format!(
        "/authen/ssoAuthorizationLogin?authorization={}&{}",
        authorization,
        s.to_query()
    )
}

pub fn path_get_message_file(app_id: &str, area_id: &str) -> String {
    format!(
        "/sdologin/getMessageFile?appId={}&areaId={}&locale={}&productId={}&productVersion={}",
        app_id, area_id, LOCALE, PRODUCT_ID, LOGIN_PRODUCT_VERSION
    )
}

pub fn path_agreement() -> String {
    format!(
        "/agreement/user?appid={}&scene=optimisepc&privacypolicyversion=3&serviceAgreementVersion=7",
        LOGIN_APP_ID
    )
}

pub fn path_face_verify_init(device: &Device, tgt0: &str) -> String {
    format!(
        "/api/faceVerify/init?authenType=1&appId={}&scene=face_login&deviceId={}&authenToken={}&areaId={}&bizVersion={}",
        LOGIN_APP_ID, device.device_id, tgt0, LOGIN_AREA_ID, LOGIN_PRODUCT_VERSION
    )
}

pub fn path_get_promotion_info(s: &Suffix, tgt0: &str) -> String {
    format!(
        "/authen/getPromotionInfo.json?tgt={}&promotionFlag=1&{}",
        tgt0,
        s.to_query()
    )
}

pub fn path_get_login_user_info(s: &Suffix, tgt0: &str) -> String {
    format!(
        "/authen/getLoginUserInfo.json?tgt={}&{}",
        tgt0,
        s.to_query()
    )
}

pub fn path_get_system_config(s: &Suffix) -> String {
    format!(
        "/authen/v2/getSystemConfig?logintype=godown&{}",
        s.to_query()
    )
}

pub fn path_server_json(ms: u128) -> String {
    format!(
        "/v3launcher/server/{}/8847/server.json?time={}",
        GAME_APP_ID, ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> Device {
        Device {
            mac_id: "00-11-22-33-44-55".into(),
            device_id: "88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:".into(),
            ep_name: "DESKTOP-ABC1234".into(),
            ep_ip: "192.168.1.100".into(),
            keep_login_key: None,
            last_area_id: None,
        }
    }

    const RTID: &str = "6EC5EF3932F14524AEC4862361942649";

    /// 公共后缀按固定顺序逐字拼装。
    #[test]
    fn login_suffix_param_order() {
        let s = Suffix::login(&dev(), RTID);
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
        let s = Suffix::login_no_group(&dev(), RTID);
        let q = s.to_query();
        assert!(!q.contains("groupId="), "{q}");
        assert!(
            q.contains("&appId=791000814&areaId=1&appIdSite=791000814&"),
            "{q}"
        );
    }

    #[test]
    fn code_key_login_prefix_order() {
        let s = Suffix::login(&dev(), RTID);
        let p = path_code_key_login(
            &s,
            "0123456789abcdef0123456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF",
            1,
        );
        assert!(
            p.starts_with(
                "/authen/codeKeyLogin.json?codeKey=0123456789abcdef0123456789abcdef\
&guid=0123456789ABCDEF0123456789ABCDEF\
&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&maxsize=128&authenSource=1&"
            ),
            "{p}"
        );
    }

    #[test]
    fn get_guid_and_code_key_prefixes() {
        let s = Suffix::login(&dev(), RTID);
        assert!(path_get_guid(&s).starts_with("/authen/getGuid.json?generateDynamicKey=1&authenSource=1&appId=791000814&areaId=1&groupId=1&appIdSite=791000814&"));
        assert!(path_get_code_key(&s)
            .starts_with("/authen/getCodeKey.json?maxsize=128&authenSource=1&"));
    }

    #[test]
    fn fast_in_login_template() {
        let s = Suffix::login_no_group(&dev(), RTID);
        let p = path_fast_in_login(&s, "ULSKLK-TEST");
        assert!(p.starts_with(
            "/authen/v2/fastInLogin?keepLoginKey=ULSKLK-TEST&authenSource=1&appId=791000814&areaId=1&appIdSite=791000814&"
        ), "{p}");
        assert!(!p.contains("groupId"), "{p}");
    }

    #[test]
    fn push_templates() {
        let s = Suffix::login(&dev(), RTID);
        assert!(path_cancel_push_message_login(&s, "G1").starts_with(
            "/authen/cancelPushMessageLogin.json?pushMsgSessionKey=&guid=G1&authenSource=1&"
        ));
        let send = path_send_push_message(&s, "user@example.com", "G1");
        assert!(send.starts_with(
            "/authen/sendPushMessage.json?inputUserId=user%40example%2Ecom&scene=pc_pushmsglogin&guid=G1&authenSource=1&"
        ), "{send}");
        let poll = path_push_message_login(&s, "S1", "G1");
        assert!(poll.starts_with(
            "/authen/pushMessageLogin.json?pushMsgSessionKey=S1&guid=G1&autoLoginFlag=0&autoLoginKeepTime=0&keepLoginFlag=1&authenSource=1&"
        ));
        assert!(!poll.contains("maxsize"), "push 轮询不得带 maxsize");
    }

    #[test]
    fn sso_legs_match_template() {
        let s1 = Suffix::for_sso_authorization(&dev(), RTID, "7");
        let p1 = path_get_sso_authorization(&s1, "ULSTGT-T0", "GUID0");
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

        let s2 = Suffix::for_sso_login(&dev(), RTID, "7");
        let p2 = path_sso_authorization_login(&s2, "UA-123");
        // `ssoAuthorizationLogin`：无 guid/tgt；epIp/runTimeId/channelId 值为空；版本 1.9.7.10
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
            p2.contains("&runTimeId=&channelId=&productVersion=1%2E9%2E7%2E10&tag=0"),
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
        let s = Suffix::login(&dev(), RTID);
        assert_eq!(
            path_get_message_file(LOGIN_APP_ID, "1"),
            "/sdologin/getMessageFile?appId=791000814&areaId=1&locale=zh_CN&productId=4&productVersion=1.1.344.45"
        );
        assert_eq!(
            path_get_message_file(GAME_APP_ID, "8"),
            "/sdologin/getMessageFile?appId=100001900&areaId=8&locale=zh_CN&productId=4&productVersion=1.1.344.45"
        );
        let fv = path_face_verify_init(&dev(), "ULSTGT-T0");
        assert_eq!(
            fv,
            "/api/faceVerify/init?authenType=1&appId=791000814&scene=face_login\
&deviceId=88440FF9DD5D5C6819D6D4652279A1BC:4F2A1C7E9B0D3568A1E4C7F02B9D6E31:\
&authenToken=ULSTGT-T0&areaId=1&bizVersion=1.1.344.45"
        );
        assert!(path_get_promotion_info(&s, "T")
            .starts_with("/authen/getPromotionInfo.json?tgt=T&promotionFlag=1&authenSource=1&"));
        assert!(path_get_login_user_info(&s, "T")
            .starts_with("/authen/getLoginUserInfo.json?tgt=T&authenSource=1&"));
        let cfg = path_get_system_config(&Suffix::login_no_group(&dev(), RTID));
        assert!(cfg.starts_with("/authen/v2/getSystemConfig?logintype=godown&authenSource=1&appId=791000814&areaId=1&appIdSite=791000814&"), "{cfg}");
        assert!(!cfg.contains("groupId"), "{cfg}");
        assert_eq!(path_agreement(), "/agreement/user?appid=791000814&scene=optimisepc&privacypolicyversion=3&serviceAgreementVersion=7");
    }

    #[test]
    fn tag_minus_one_skips_param() {
        let mut s = Suffix::login(&dev(), RTID);
        s.tag = -1;
        assert!(!s.to_query().contains("&tag="));
        // channelId 非零：只 WARN，不改模板
        let mut s2 = Suffix::login(&dev(), RTID);
        s2.channel_id = "3".into();
        assert!(s2.to_query().contains("&channelId=3&"));
    }

    #[test]
    fn server_json_path_shape() {
        assert_eq!(
            path_server_json(1759400000123),
            "/v3launcher/server/100001900/8847/server.json?time=1759400000123"
        );
    }
}
