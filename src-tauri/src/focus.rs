//! 目标窗口的焦点探测：给待命门闩判断「焦点还是不是用户确认过的那个窗口」。
//!
//! 三个平台分支：
//!
//! * **macOS**：用系统级 AX API 读 `AXFocusedApplication` → `AXFocusedWindow`（**窗口级**），
//!   并用窗口的位置 + 尺寸作为稳定标识；AX 不可用（未授予「辅助功能」/ 查询失败）时回退到
//!   `NSWorkspace.frontmostApplication`（**应用级**，不需要任何权限）。
//! * **Windows**：`GetForegroundWindow` + `GetWindowThreadProcessId` + 窗口类名（窗口级）。
//! * **其它平台**：恒 [`FocusProbe::Unavailable`] —— 待命按「焦点未变化」处理，与不检测一致。
//!
//! # 为什么标识里不含窗口标题
//!
//! 标题会随内容变化（浏览器切标签、文档改名），拿它做相等比较会产生大量误判；因此
//! macOS 用「位置 + 尺寸」，Windows 用 `HWND + 类名`，标题只用于日志与提示。
//!
//! # 线程
//!
//! `probe()` 会被打字循环（Worker 的阻塞线程）调用。AX 的 C API 可以在任意线程调用；
//! `NSWorkspace` 回退路径只在 AX 不可用时才走到，且只读取值。

use std::process;

/// 焦点探测结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusProbe {
    /// 焦点在别的应用/窗口上：这就是用户选定的目标。
    Target(FocusSignature),
    /// 焦点在我们自己的窗口上（待命/主/进度/脚本窗口，或我们自己弹出的系统框）。
    SelfApp,
    /// 读不到（平台不支持 / 权限不足 / 查询失败）。调用方按「未变化」处理（fail-open）。
    Unavailable,
}

/// 目标窗口的身份：只有 `pid` + `key` 参与比较。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusSignature {
    /// 窗口的属主进程。
    pid: i32,
    /// 人类可读描述（日志 / 提示文案），**不参与**比较。
    label: String,
    /// 参与比较的稳定标识（平台相关）。
    key: String,
}

impl FocusSignature {
    pub(crate) fn new(pid: i32, label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            pid,
            label: label.into(),
            key: key.into(),
        }
    }

    /// 是不是同一个目标窗口（`label` 变化——例如窗口标题变了——不算变化）。
    pub fn same_target(&self, other: &Self) -> bool {
        self.pid == other.pid && self.key == other.key
    }

    /// 面向用户/日志的描述，例如窗口标题或应用名。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 窗口的属主进程。
    pub fn pid(&self) -> i32 {
        self.pid
    }
}

/// 当前焦点在哪里。
pub fn probe() -> FocusProbe {
    platform::probe()
}

/// 目标窗口是不是我们自己（待命/主/进度/脚本窗口，或我们弹的系统框）。
fn is_self(pid: i32) -> bool {
    pid == process::id() as i32
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::OnceLock;

    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use objc2_app_kit::NSWorkspace;

    use super::{is_self, FocusProbe, FocusSignature};

    type AXUIElementRef = *mut c_void;
    type AXValueRef = *mut c_void;
    type AXError = i32;
    type Boolean = u8;

    const AX_ERROR_SUCCESS: AXError = 0;
    /// `kAXValueCGPointType` / `kAXValueCGSizeType`（见 AXValue.h）。
    const AX_VALUE_CG_POINT: u32 = 1;
    const AX_VALUE_CG_SIZE: u32 = 2;
    /// 单个 AX 请求的超时（秒）：目标 App 卡死时不要把打字线程拖住。
    const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.05;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
        fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> Boolean;
        fn AXIsProcessTrusted() -> Boolean;
    }

    pub fn probe() -> FocusProbe {
        if trusted() {
            if let Some(probe) = ax_probe() {
                return probe;
            }
        }
        app_level_probe()
    }

    /// 「辅助功能」权限是否已授予（AX 查询的前提）。
    ///
    /// 缓存结果：TCC 的变更本来也需要重启应用才对键盘注入生效。
    fn trusted() -> bool {
        static TRUSTED: OnceLock<bool> = OnceLock::new();
        *TRUSTED.get_or_init(|| {
            // SAFETY: 无参数、无副作用的权限查询。
            unsafe { AXIsProcessTrusted() != 0 }
        })
    }

    /// 窗口级探测：系统级 AX → 焦点应用 → 焦点窗口 → (pid, 标题, 位置+尺寸)。
    ///
    /// 任何一步失败都返回 `None`，由调用方回退到应用级。
    fn ax_probe() -> Option<FocusProbe> {
        // SAFETY: 所有指针都是 AX/CoreFoundation 返回的对象；`owned`/`copy_attr` 用
        // `wrap_under_create_rule` 接管 +1 引用，作用域结束自动 CFRelease。
        unsafe {
            let system = owned(AXUIElementCreateSystemWide())?;
            let app = copy_attr(as_element(&system), "AXFocusedApplication")?;
            let app_element = as_element(&app);

            let mut app_pid = -1;
            if AXUIElementGetPid(app_element, &mut app_pid) != AX_ERROR_SUCCESS {
                return None;
            }
            if is_self(app_pid) {
                return Some(FocusProbe::SelfApp);
            }
            // 目标 App 卡死时的兜底超时（必须在读属性前设置）
            AXUIElementSetMessagingTimeout(app_element, AX_MESSAGING_TIMEOUT_SECS);

            let window = copy_attr(app_element, "AXFocusedWindow")?;
            let window_element = as_element(&window);

            let mut window_pid = -1;
            if AXUIElementGetPid(window_element, &mut window_pid) != AX_ERROR_SUCCESS {
                return None;
            }
            if is_self(window_pid) {
                return Some(FocusProbe::SelfApp);
            }

            // 位置 + 尺寸拿不到就没有稳定的窗口标识 → 交给应用级回退
            let key = geometry_key(window_element)?;
            let title = copy_attr(window_element, "AXTitle")
                .map(|value| cfstring_to_string(value.as_concrete_TypeRef()))
                .unwrap_or_default();

            let label = if title.trim().is_empty() {
                format!("pid {window_pid}")
            } else {
                title
            };
            Some(FocusProbe::Target(FocusSignature::new(
                window_pid, label, key,
            )))
        }
    }

    /// 应用级回退：`NSWorkspace.frontmostApplication`（不需要任何权限）。
    fn app_level_probe() -> FocusProbe {
        let front = objc2::rc::autoreleasepool(|_pool| {
            let workspace = NSWorkspace::sharedWorkspace();
            let front = workspace.frontmostApplication()?;
            let pid = front.processIdentifier();
            let bundle = front.bundleIdentifier().map(|value| value.to_string());
            let name = front.localizedName().map(|value| value.to_string());
            Some((pid, bundle, name))
        });

        match front {
            Some((pid, _, _)) if is_self(pid) => FocusProbe::SelfApp,
            Some((pid, bundle, name)) => {
                let label = name
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("pid {pid}"));
                let key = bundle
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("pid {pid}"));
                FocusProbe::Target(FocusSignature::new(pid, label, key))
            }
            None => FocusProbe::Unavailable,
        }
    }

    /// 接管一个 +1 引用的 CF 对象（空指针视为失败）。
    fn owned(reference: CFTypeRef) -> Option<CFType> {
        if reference.is_null() {
            return None;
        }
        // SAFETY: 调用方保证该对象是 +1 引用（`CF_RETURNS_RETAINED` 或 Create/Copy 函数）。
        Some(unsafe { CFType::wrap_under_create_rule(reference) })
    }

    fn as_element(object: &CFType) -> AXUIElementRef {
        object.as_concrete_TypeRef() as AXUIElementRef
    }

    /// 读一个 AX 属性；失败或为空返回 `None`（返回的对象由 `CFType` 持有并在 Drop 时释放）。
    fn copy_attr(element: AXUIElementRef, attribute: &str) -> Option<CFType> {
        let attribute = CFString::new(attribute);
        let mut value: CFTypeRef = ptr::null();
        // SAFETY: `element` 是有效的 AX 元素；`attribute` 是 CFString；`value` 是出参。
        // 成功时 AX 返回 +1 引用（`CF_RETURNS_RETAINED`），交给 `owned` 接管。
        let error = unsafe {
            AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
        };
        if error != AX_ERROR_SUCCESS {
            return None;
        }
        owned(value)
    }

    /// 窗口的「位置 + 尺寸」作为稳定标识（四舍五入到整数点，避免浮点相等问题）。
    fn geometry_key(window: AXUIElementRef) -> Option<String> {
        let position = copy_attr(window, "AXPosition")?;
        let size = copy_attr(window, "AXSize")?;

        let mut point = CGPoint::default();
        let mut cg_size = CGSize::default();
        // SAFETY: 传入的 AXValue 类型与 `the_type` 匹配（AXPosition → CGPoint，AXSize → CGSize）。
        let (ok_point, ok_size) = unsafe {
            (
                AXValueGetValue(
                    position.as_concrete_TypeRef() as AXValueRef,
                    AX_VALUE_CG_POINT,
                    (&mut point as *mut CGPoint).cast::<c_void>(),
                ),
                AXValueGetValue(
                    size.as_concrete_TypeRef() as AXValueRef,
                    AX_VALUE_CG_SIZE,
                    (&mut cg_size as *mut CGSize).cast::<c_void>(),
                ),
            )
        };
        if ok_point == 0 || ok_size == 0 {
            return None;
        }

        Some(format!(
            "{},{},{},{}",
            point.x.round() as i64,
            point.y.round() as i64,
            cg_size.width.round() as i64,
            cg_size.height.round() as i64
        ))
    }

    /// CFString → Rust `String`（不接管所有权：调用方仍持有该对象）。
    fn cfstring_to_string(reference: CFTypeRef) -> String {
        if reference.is_null() {
            return String::new();
        }
        // SAFETY: AX 的 `AXTitle` 是 CFString；这里只读它，所有权仍归调用方。
        let string = unsafe { CFString::wrap_under_get_rule(reference as CFStringRef) };
        string.to_string()
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    use super::{is_self, FocusProbe, FocusSignature};

    pub fn probe() -> FocusProbe {
        // SAFETY: 都是只读的前台窗口查询；两个缓冲区由我们提供并给出正确长度。
        unsafe {
            let hwnd: HWND = GetForegroundWindow();
            if hwnd.is_null() {
                return FocusProbe::Unavailable;
            }

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if is_self(pid as i32) {
                return FocusProbe::SelfApp;
            }

            let mut class_buffer = [0u16; 256];
            let class_len =
                GetClassNameW(hwnd, class_buffer.as_mut_ptr(), class_buffer.len() as i32);
            let class = String::from_utf16_lossy(&class_buffer[..class_len.max(0) as usize]);

            let mut title_buffer = [0u16; 512];
            let title_len =
                GetWindowTextW(hwnd, title_buffer.as_mut_ptr(), title_buffer.len() as i32);
            let title = String::from_utf16_lossy(&title_buffer[..title_len.max(0) as usize]);

            let label = if title.trim().is_empty() {
                class.clone()
            } else {
                title
            };
            FocusProbe::Target(FocusSignature::new(
                pid as i32,
                label,
                format!("{:x}|{class}", hwnd as usize),
            ))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::FocusProbe;

    /// 其它平台不探测：待命按「焦点未变化」处理，行为与不检测一致。
    pub fn probe() -> FocusProbe {
        FocusProbe::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_target_ignores_label() {
        let a = FocusSignature::new(42, "记事本", "10,20,300,400");
        let b = FocusSignature::new(42, "未命名 - 记事本", "10,20,300,400");
        assert!(a.same_target(&b), "标题变了但窗口没变，应当算同一个目标");
        assert_eq!(a.label(), "记事本");
        assert_eq!(a.pid(), 42);
    }

    #[test]
    fn same_target_detects_window_and_process_change() {
        let base = FocusSignature::new(42, "记事本", "10,20,300,400");
        let moved = FocusSignature::new(42, "记事本", "11,20,300,400");
        let other_app = FocusSignature::new(43, "记事本", "10,20,300,400");
        assert!(!base.same_target(&moved), "窗口位置变了应当算换了目标");
        assert!(!base.same_target(&other_app), "换了进程应当算换了目标");
    }

    #[test]
    fn self_detection_uses_own_pid() {
        assert!(is_self(std::process::id() as i32));
        assert!(!is_self(std::process::id() as i32 + 1));
    }
}
