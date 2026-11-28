//! 单实例守卫。
//!
//! 背景：应用原来允许多开。每开一次就多一个进程、多一份 WebView2 用户数据目录占用，
//! 关闭时只要有一个实例没退干净，`archive-gallery.exe` 就被锁住，
//! 重新打包会失败、装完新版打开看到的还是旧界面。
//!
//! 做法：启动时先抢一个命名互斥体。
//! - 抢到 → 本进程是唯一实例，把守卫交给 Tauri 托管，活到进程结束；
//! - 抢不到 → 已有实例在跑，把它的窗口恢复并置前，然后本进程立刻退出。
//!
//! 不引 `tauri-plugin-single-instance`：该插件目前只有 3.0.0-alpha 预发布版，
//! 而这里需要的语义只有「独占 + 唤醒已有窗口」，用 Win32 原语更稳、也没有新依赖风险。

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsIconic,
        IsWindowVisible, PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    /// 互斥体名字。`Local\` 前缀表示只在当前登录会话内互斥，
    /// 多用户同机时互不干扰，也比 `Global\` 少一类权限问题。
    const MUTEX_NAME: &str = "Local\\com.archivegallery.desktop.single-instance";

    /// 唤醒已有窗口用的自定义消息（WM_APP + 1 起是安全的自定义区间）。
    const WM_SHOW_INSTANCE: u32 = 0x8000 + 1;

    /// 主窗口标题前缀，取自 `tauri.conf.json` 的 `title`。
    const WINDOW_TITLE_PREFIX: &str = "画匣";

    /// 主窗的完整标题（`tauri.conf.json` → `app.windows[0].title`）。
    ///
    /// 光按前缀「画匣」匹配会误伤拆出的独立窗（`画匣 · 独立标签`）：
    /// 两个窗都以「画匣」开头，EnumWindows 先撞上哪个就把哪个提到前面，
    /// 用户点桌面图标想回主窗主页，结果弹的是拆出去的标签窗
    /// （2026-11-28 实机反馈）。所以优先精确匹配主窗标题。
    const MAIN_WINDOW_TITLE: &str = "画匣 · 本地画廊";

    /// 持有就代表本进程是唯一实例；Drop 时释放。
    pub struct InstanceGuard {
        handle: HANDLE,
    }

    // 互斥体句柄可以被任意线程接管，声明成 Send/Sync 是安全的。
    unsafe impl Send for InstanceGuard {}
    unsafe impl Sync for InstanceGuard {}

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe { CloseHandle(self.handle) };
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 尝试成为唯一实例。`Some` = 抢到了，可以继续启动；`None` = 已有实例。
    pub fn acquire() -> Option<InstanceGuard> {
        let name = wide(MUTEX_NAME);
        // null 的 SECURITY_ATTRIBUTES 即默认安全描述符。
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };

        if handle.is_null() {
            // 建不出互斥体（极少见：句柄耗尽）。宁可让用户能用，
            // 也不要因为守卫失败而开不了应用。
            eprintln!("单实例互斥体创建失败，按未加守卫处理");
            return None;
        }

        // CreateMutexW 在「名字已存在」时同样返回有效句柄，
        // 必须看 GetLastError 才能区分「我创建的」和「别人已创建」。
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already {
            unsafe { CloseHandle(handle) };
            return None;
        }

        Some(InstanceGuard { handle })
    }

    struct Search {
        /// 精确命中主窗（最高优先级，命中即停）。
        exact: *mut c_void,
        visible: *mut c_void,
        any: *mut c_void,
    }

    unsafe extern "system" fn enum_proc(hwnd: *mut c_void, lparam: isize) -> i32 {
        let search = unsafe { &mut *(lparam as *mut Search) };
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if length <= 0 {
            return 1;
        }
        let mut buffer = vec![0u16; (length + 1) as usize];
        let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), length + 1) };
        if copied <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buffer[..copied as usize]);
        // 主窗优先：不管可见与否都认它（老板键藏起来时也要能唤回）。
        if title == MAIN_WINDOW_TITLE {
            search.exact = hwnd;
            return 0;
        }
        if title.starts_with(WINDOW_TITLE_PREFIX) {
            if unsafe { IsWindowVisible(hwnd) } != 0 {
                if search.visible.is_null() {
                    search.visible = hwnd; // 记下可见的，但继续找主窗
                }
                return 1;
            }
            if search.any.is_null() {
                search.any = hwnd; // 先记下不可见的，继续找可见的
            }
        }
        1
    }

    /// 让已在运行的那个实例把窗口显示出来并置前。
    ///
    /// 不按窗口类名查找：Tauri 2 的窗口类名是内部分配的，不是产品名。
    /// 这里用 `EnumWindows` 按标题前缀匹配。
    pub fn wake_existing() -> bool {
        let mut search = Search {
            exact: std::ptr::null_mut(),
            visible: std::ptr::null_mut(),
            any: std::ptr::null_mut(),
        };
        unsafe {
            EnumWindows(Some(enum_proc), &mut search as *mut Search as isize);
        }

        // 主窗 > 可见的「画匣*」窗 > 任意「画匣*」窗。
        let target = if !search.exact.is_null() {
            search.exact
        } else if !search.visible.is_null() {
            search.visible
        } else if !search.any.is_null() {
            search.any
        } else {
            return false;
        };

        unsafe {
            // 最小化的先还原，隐藏的（老板键）直接 show——两种都覆盖。
            if IsIconic(target) != 0 {
                ShowWindow(target, SW_RESTORE);
            } else {
                ShowWindow(target, SW_SHOW);
            }
            // SetForegroundWindow 受前台权限限制常静默失败，补 BringWindowToTop
            // 把窗口提到 Z 序顶部，至少不会被别的窗盖住（2026-11-28 反馈：
            // 点桌面图标窗口没到前面，看着像「没反应」）。
            SetForegroundWindow(target);
            BringWindowToTop(target);
            // 也用消息通知对方自己 set_focus：对方进程处理消息时置前更容易成功。
            PostMessageW(target, WM_SHOW_INSTANCE, 0, 0);
        }
        true
    }
}

#[cfg(not(windows))]
mod platform {
    pub struct InstanceGuard;

    pub fn acquire() -> Option<InstanceGuard> {
        Some(InstanceGuard)
    }

    pub fn wake_existing() -> bool {
        false
    }
}

pub use platform::{acquire, wake_existing, InstanceGuard};
