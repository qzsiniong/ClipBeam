//! 系统通知（macOS：通知中心；Windows：Toast；其他平台静默 no-op）。
//! 所有失败都吞掉——通知是辅助反馈，不能影响主流程。

/// 弹出一条系统通知。
pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        use mac_notification_sys::{set_application, Notification};
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            // 无 bundle 的裸二进制会让 mac-notification-sys 用 AppleScript 查找
            // 名为 "use_default" 的应用，找不到时弹出 "Choose Application" 对话框。
            // 主动绑定到一个一定存在的 bundle ID，避免弹窗。
            let _ = set_application("com.apple.finder");
        });
        let _ = Notification::new().title(title).message(body).send();
    }

    #[cfg(target_os = "windows")]
    {
        use winrt_notification::Toast;
        let _ = Toast::new(Toast::POWERSHELL_APP_ID)
            .title(title)
            .text1(body)
            .show();
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body);
    }
}
