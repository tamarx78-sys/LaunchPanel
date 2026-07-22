use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

#[cfg(debug_assertions)]
const MUTEX_NAME: &str = "Local\\LaunchPanel_Debug_Mutex";
#[cfg(debug_assertions)]
const SHOW_EVENT_NAME: &str = "Local\\LaunchPanel_Debug_Show";

#[cfg(not(debug_assertions))]
const MUTEX_NAME: &str = "Local\\LaunchPanel_Mutex";
#[cfg(not(debug_assertions))]
const SHOW_EVENT_NAME: &str = "Local\\LaunchPanel_Show";

pub enum Acquisition {
    Primary(SingleInstance),
    Existing,
}

pub struct SingleInstance {
    _mutex: OwnedHandle,
    show_event: OwnedHandle,
}

struct OwnedHandle(HANDLE);

// Kernel object handles are valid process-wide, so ownership may be transferred
// to the listener thread.
unsafe impl Send for OwnedHandle {}

pub fn acquire() -> windows::core::Result<Acquisition> {
    let mutex_name = HSTRING::from(MUTEX_NAME);
    let show_event_name = HSTRING::from(SHOW_EVENT_NAME);

    // The last-error value must be read immediately after CreateMutexW.
    let mutex = unsafe { CreateMutexW(None, false, &mutex_name)? };
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    let show_event = match unsafe { CreateEventW(None, false, false, &show_event_name) } {
        Ok(event) => event,
        Err(error) => {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return Err(error);
        }
    };

    if already_running {
        let signal_result = unsafe { SetEvent(show_event) };
        unsafe {
            let _ = CloseHandle(show_event);
            let _ = CloseHandle(mutex);
        }
        signal_result?;
        Ok(Acquisition::Existing)
    } else {
        Ok(Acquisition::Primary(SingleInstance {
            _mutex: OwnedHandle(mutex),
            show_event: OwnedHandle(show_event),
        }))
    }
}

impl SingleInstance {
    pub fn listen<F>(self, mut on_show_requested: F)
    where
        F: FnMut() + Send + 'static,
    {
        std::thread::spawn(move || {
            loop {
                if !self.wait_for_show_request() {
                    break;
                }
                on_show_requested();
            }
        });
    }

    fn wait_for_show_request(&self) -> bool {
        // Keeping self alive also keeps the mutex owned for this process.
        unsafe { WaitForSingleObject(self.show_event.0, INFINITE) == WAIT_OBJECT_0 }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
