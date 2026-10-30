use anyhow::bail;
use windows::core::IUnknown;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiElementId(Vec<i32>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub window: HWND,
    pub element: Option<UiElementId>,
    pub segment_id: u64,
}

pub fn capture() -> anyhow::Result<Target> {
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        bail!("没有前台窗口");
    }
    Ok(Target {
        window,
        element: current_runtime_id(),
        segment_id: 0,
    })
}

pub fn focused(target: &Target) -> bool {
    if target.window.0.is_null() {
        return false;
    }
    if !unsafe { IsWindow(target.window) }.as_bool() {
        return false;
    }
    if unsafe { GetForegroundWindow() } != target.window {
        return false;
    }
    match &target.element {
        Some(expected) => match current_runtime_id() {
            Some(actual) => &actual == expected,
            None => true,
        },
        None => true,
    }
}

#[allow(dead_code)]
pub fn same(a: &Target, b: &Target) -> bool {
    a.window == b.window && a.element == b.element
}

fn current_runtime_id() -> Option<UiElementId> {
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        let element = automation.GetFocusedElement().ok()?;
        let array = element.GetRuntimeId().ok()?;
        if array.is_null() {
            return None;
        }
        let result = (|| -> Option<UiElementId> {
            let lower = SafeArrayGetLBound(array, 1).ok()?;
            let upper = SafeArrayGetUBound(array, 1).ok()?;
            let count = upper.checked_sub(lower)?.checked_add(1)? as usize;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).ok()?;
            let values = std::slice::from_raw_parts(data as *const i32, count).to_vec();
            let _ = SafeArrayUnaccessData(array);
            Some(UiElementId(values))
        })();
        let _ = SafeArrayDestroy(array);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_compares_window_and_element() {
        let window = HWND::default();
        let first = Target {
            window,
            element: Some(UiElementId(vec![1, 2])),
            segment_id: 3,
        };
        let second = Target {
            segment_id: 9,
            ..first.clone()
        };
        assert!(same(&first, &second));
        let third = Target {
            element: Some(UiElementId(vec![1, 3])),
            ..second
        };
        assert!(!same(&first, &third));
    }
}
