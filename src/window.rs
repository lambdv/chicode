//! Windows-only hit testing for the frameless Slint title bar and resize edges.

use crate::AppWindow;
use slint::ComponentHandle;
use slint::winit_030::{WinitWindowAccessor, winit};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTRIGHT,
            HTTOP, HTTOPLEFT, HTTOPRIGHT, WM_NCDESTROY, WM_NCHITTEST,
        },
    },
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Install native caption and resize hit tests once winit creates the HWND.
pub fn install(ui: &AppWindow) -> Result<(), Box<dyn std::error::Error>> {
    let Some(result) = ui.window().with_winit_window(|window| {
        let handle = window.window_handle()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err("Expected a Win32 window handle".into());
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        // The subclass lives on the UI thread and releases this weak handle on WM_NCDESTROY.
        let data = Box::into_raw(Box::new(ui.as_weak())) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(hit_test), 1, data).as_bool() } {
            Ok(())
        } else {
            unsafe { drop(Box::from_raw(data as *mut slint::Weak<AppWindow>)) };
            Err("Cannot install window hit testing".into())
        }
    }) else {
        return Err("Winit window is not ready".into());
    };
    result
}

/// Leave interactive controls to Slint, but let Windows drag and resize the chrome.
unsafe extern "system" fn hit_test(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        // Forward first so winit can finish destroying its window.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(hit_test), id);
            drop(Box::from_raw(data as *mut slint::Weak<AppWindow>));
        }
        return result;
    }
    if message == WM_NCHITTEST {
        let weak = unsafe { &*(data as *const slint::Weak<AppWindow>) };
        if let Some(ui) = weak.upgrade() {
            let mut bounds = RECT::default();
            let mut point = POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            if unsafe {
                GetClientRect(hwnd, &mut bounds).is_ok()
                    && ScreenToClient(hwnd, &mut point).as_bool()
            } {
                let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
                let edge = (6.0 * scale).round() as i32;
                let left = point.x < edge;
                let right = point.x >= bounds.right - edge;
                let top = point.y < edge;
                let bottom = point.y >= bounds.bottom - edge;

                // Native edges provide resize cursors and preserve Windows snap/resize behavior.
                if !ui.window().is_maximized() {
                    let part = match (left, right, top, bottom) {
                        (true, _, true, _) => HTTOPLEFT,
                        (_, true, true, _) => HTTOPRIGHT,
                        (true, _, _, true) => HTBOTTOMLEFT,
                        (_, true, _, true) => HTBOTTOMRIGHT,
                        (true, _, _, _) => HTLEFT,
                        (_, true, _, _) => HTRIGHT,
                        (_, _, true, _) => HTTOP,
                        (_, _, _, true) => HTBOTTOM,
                        _ => 0,
                    };
                    if part != 0 {
                        return LRESULT(part as isize);
                    }
                }
                // The title ends before the model picker; controls remain HTCLIENT.
                if point.y >= 0
                    && (point.y as f32) < 44.0 * scale * ui.get_zoom_factor()
                    && point.x >= 0
                    && (point.x as f32) < ui.get_drag_boundary() * scale * ui.get_zoom_factor()
                    && ((point.x as f32) - ui.get_navigation_width() * scale * ui.get_zoom_factor())
                        .abs()
                        >= 4.0 * scale * ui.get_zoom_factor()
                {
                    return LRESULT(HTCAPTION as isize);
                }
            }
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
