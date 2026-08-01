use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, UIA_ListItemControlTypeId};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GA_ROOT, GetAncestor, GetClassNameW, GetMessageW, GetParent,
    GetSystemMetrics, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SM_CXDOUBLECLK,
    SM_CYDOUBLECLK, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_QUIT, WindowFromPoint,
};

const QUEUE_CAPACITY: usize = 64;

thread_local! {
    static EVENT_SENDER: RefCell<Option<SyncSender<MouseEvent>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
struct MouseEvent {
    message: u32,
    point: POINT,
    time: u32,
    flags: u32,
}

pub struct DesktopDoubleClickHook {
    hook_thread_id: u32,
    hook_thread: Option<JoinHandle<()>>,
    worker_thread: Option<JoinHandle<()>>,
}

impl DesktopDoubleClickHook {
    pub fn start<F>(on_double_click: F) -> Result<Self, String>
    where
        F: Fn() + Send + 'static,
    {
        let (event_sender, event_receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = mpsc::channel();

        let worker_thread = thread::spawn(move || worker_loop(event_receiver, on_double_click));
        let hook_thread = thread::spawn(move || hook_loop(event_sender, ready_sender));

        match ready_receiver.recv() {
            Ok(Ok(hook_thread_id)) => Ok(Self {
                hook_thread_id,
                hook_thread: Some(hook_thread),
                worker_thread: Some(worker_thread),
            }),
            Ok(Err(error)) => {
                let _ = hook_thread.join();
                let _ = worker_thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = hook_thread.join();
                let _ = worker_thread.join();
                Err("マウスフックスレッドの初期化に失敗しました".to_string())
            }
        }
    }
}

impl Drop for DesktopDoubleClickHook {
    fn drop(&mut self) {
        // SAFETY: The id belongs to the live hook thread. WM_QUIT makes it unhook and exit.
        let _ = unsafe { PostThreadMessageW(self.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.hook_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.worker_thread.take() {
            let _ = thread.join();
        }
    }
}

fn hook_loop(sender: SyncSender<MouseEvent>, ready: mpsc::Sender<Result<u32, String>>) {
    EVENT_SENDER.with(|slot| *slot.borrow_mut() = Some(sender));

    // SAFETY: The callback has the required ABI, and this thread owns the message loop.
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) };
    let hook = match hook {
        Ok(hook) => hook,
        Err(error) => {
            EVENT_SENDER.with(|slot| slot.borrow_mut().take());
            let _ = ready.send(Err(format!("マウスフックを登録できませんでした: {error}")));
            return;
        }
    };

    // Force creation of this thread's message queue before publishing its id.
    let mut msg = MSG::default();
    // SAFETY: A null HWND requests this thread's queue without removing a message.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
            &mut msg,
            None,
            0,
            0,
            windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE,
        );
    }
    // SAFETY: Called on the current thread.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        // SAFETY: msg is valid for the duration of the call.
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // SAFETY: msg was populated by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // SAFETY: hook was installed successfully by this thread and is unhooked exactly once.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
    EVENT_SENDER.with(|slot| slot.borrow_mut().take());
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && (wparam.0 as u32 == WM_LBUTTONDOWN || wparam.0 as u32 == WM_MOUSEMOVE) {
        // SAFETY: Windows supplies an MSLLHOOKSTRUCT pointer for a nonnegative hook code.
        let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let event = MouseEvent {
            message: wparam.0 as u32,
            point: data.pt,
            time: data.time,
            flags: data.flags,
        };
        EVENT_SENDER.with(|slot| {
            if let Some(sender) = slot.borrow().as_ref() {
                match sender.try_send(event) {
                    Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
                }
            }
        });
    }

    // SAFETY: Passing the event onward is required for a low-level hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[derive(Clone, Copy)]
struct FirstClick {
    point: POINT,
    time: u32,
    desktop_region: usize,
    dragged: bool,
}

fn worker_loop<F>(receiver: Receiver<MouseEvent>, on_double_click: F)
where
    F: Fn(),
{
    let automation = Automation::new();
    let mut first_click: Option<FirstClick> = None;

    while let Ok(event) = receiver.recv() {
        if event.flags & LLMHF_INJECTED != 0 {
            first_click = None;
            continue;
        }

        if event.message == WM_MOUSEMOVE {
            if let Some(first) = first_click.as_mut()
                && outside_double_click_rect(first.point, event.point)
            {
                first.dragged = true;
            }
            continue;
        }

        let Some(desktop_region) = desktop_blank_region(event.point, automation.as_ref()) else {
            first_click = None;
            continue;
        };

        let matched = first_click.is_some_and(|first| {
            !first.dragged
                && first.desktop_region == desktop_region
                && event.time.wrapping_sub(first.time) <= double_click_time()
                && !outside_double_click_rect(first.point, event.point)
        });

        if matched {
            first_click = None;
            on_double_click();
        } else {
            first_click = Some(FirstClick {
                point: event.point,
                time: event.time,
                desktop_region,
                dragged: false,
            });
        }
    }
}

fn double_click_time() -> u32 {
    // SAFETY: This function has no preconditions.
    unsafe { GetDoubleClickTime() }
}

fn outside_double_click_rect(first: POINT, current: POINT) -> bool {
    // SAFETY: These system metrics are always available on supported Windows versions.
    let width = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }.max(1);
    let height = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }.max(1);
    (current.x - first.x).abs() > width / 2 || (current.y - first.y).abs() > height / 2
}

struct Automation(IUIAutomation);

impl Automation {
    fn new() -> Option<Self> {
        // SAFETY: The worker owns this COM apartment until Automation is dropped.
        if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
            return None;
        }
        // SAFETY: CUIAutomation is an in-process COM class implementing IUIAutomation.
        let automation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) };
        match automation {
            Ok(automation) => Some(Self(automation)),
            Err(_) => {
                // SAFETY: Balances the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
                None
            }
        }
    }

    fn point_is_icon(&self, point: POINT) -> Option<bool> {
        // SAFETY: UI Automation accepts screen coordinates and owns the returned element.
        let element = unsafe { self.0.ElementFromPoint(point) }.ok()?;
        // SAFETY: Reading the current property is valid for a live automation element.
        let control_type = unsafe { element.CurrentControlType() }.ok()?;
        Some(control_type == UIA_ListItemControlTypeId)
    }
}

impl Drop for Automation {
    fn drop(&mut self) {
        // SAFETY: Balances this worker thread's successful CoInitializeEx.
        unsafe { CoUninitialize() };
    }
}

fn desktop_blank_region(point: POINT, automation: Option<&Automation>) -> Option<usize> {
    // SAFETY: WindowFromPoint accepts virtual-screen coordinates, including other monitors.
    let target = unsafe { WindowFromPoint(point) };
    if target.0.is_null() {
        return None;
    }

    let hierarchy = window_hierarchy(target);
    let target_is_list = hierarchy
        .first()
        .is_some_and(|name| name == "SysListView32");
    let has_def_view = hierarchy.iter().any(|name| name == "SHELLDLL_DefView");
    let root = unsafe { GetAncestor(target, GA_ROOT) };
    let root_class = class_name(root);
    let desktop_root = root_class == "Progman" || root_class == "WorkerW";

    let is_icon = automation?.point_is_icon(point)?;
    let blank = target_is_list && has_def_view && desktop_root && !is_icon;

    #[cfg(debug_assertions)]
    eprintln!(
        "desktop-click hwnd={:?} root={:?} hierarchy={hierarchy:?} icon={is_icon} blank={blank}",
        target.0, root.0
    );

    blank.then_some(target.0 as usize)
}

fn window_hierarchy(mut window: windows::Win32::Foundation::HWND) -> Vec<String> {
    let mut hierarchy = Vec::with_capacity(5);
    for _ in 0..8 {
        if window.0.is_null() {
            break;
        }
        hierarchy.push(class_name(window));
        let Ok(parent) = (unsafe { GetParent(window) }) else {
            break;
        };
        if parent == window {
            break;
        }
        window = parent;
    }
    hierarchy
}

fn class_name(window: windows::Win32::Foundation::HWND) -> String {
    if window.0.is_null() {
        return String::new();
    }
    let mut buffer = [0_u16; 128];
    // SAFETY: buffer is writable, and window is only used as an opaque handle.
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}
