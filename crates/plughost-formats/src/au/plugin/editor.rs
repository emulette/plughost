//! Editor views. A version 2 unit's Cocoa view comes from the view factory the unit names, new
//! for every opening: the v2 bridge hands out its view controller once, and that view does not
//! survive being taken out of its window. Other units give their view controller on request.
//! Units change the size of their views themselves, so an open editor's size is followed.
use std::ffi::{CString, c_void};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{NSView, NSViewController};
use objc2_audio_toolbox::{
    AUAudioUnitV2Bridge, AudioUnitCocoaViewInfo, AudioUnitGetProperty, AudioUnitGetPropertyInfo,
    kAudioUnitProperty_CocoaUI, kAudioUnitScope_Global,
};
use objc2_core_audio_kit::{AUAudioUnit_ViewController, AUViewControllerBase};
use objc2_core_foundation::{CFBundle, CFRetained, CFString, CFURL};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::{AuError, Handoff, Plugin, Slot, lock};
use crate::ResizeRequest;

/// An open editor: its view in the host's window, and the view controller that owns the view.
pub(super) struct Editor {
    view: Retained<NSView>,
    controller: Option<Retained<NSViewController>>,
    /// The view size the host last gave its window.
    size: NSSize,
    /// The view controller's preferred content size when last read.
    preferred: NSSize,
    resize: ResizeRequest,
}

impl Plugin {
    /// Adds the Audio Unit's view (a v3 view, or a v2 Cocoa view) to `parent`. [`Plugin::idle`]
    /// passes the view's later sizes to `resize`.
    ///
    /// # Safety
    ///
    /// `parent` must be a valid `NSView` that outlives the editor; call on the main thread.
    pub(crate) unsafe fn open_editor(
        &mut self,
        parent: *mut c_void,
        resize: ResizeRequest,
    ) -> Result<(u32, u32), AuError> {
        self.close_editor();
        MainThreadMarker::new().ok_or(AuError::NoEditor)?;
        let (view, controller, preferred) = match self.cocoa_view()? {
            Some(view) => (view, None, NSSize::new(0.0, 0.0)),
            None => {
                let controller = self.view_controller()?;
                let preferred = controller.preferredContentSize();
                (controller.view(), Some(controller), preferred)
            }
        };
        let size = if has_area(preferred) {
            preferred
        } else {
            view.frame().size
        };
        view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
        let parent = unsafe { &*(parent as *const NSView) };
        // The window follows the view; resizing it must not resize the view in turn.
        parent.setAutoresizesSubviews(false);
        parent.addSubview(&view);
        self.editor = Some(Editor {
            view,
            controller,
            size,
            preferred,
            resize,
        });
        Ok(pixels(size))
    }

    /// Passes a new size of the open editor's view to its window: the size a v3 view controller
    /// prefers when that changes, or the size a view gave itself.
    pub(crate) fn idle(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(controller) = &editor.controller {
            let preferred = controller.preferredContentSize();
            if preferred != editor.preferred {
                editor.preferred = preferred;
                if has_area(preferred) {
                    editor.view.setFrameSize(preferred);
                }
            }
        }
        let size = editor.view.frame().size;
        if size != editor.size {
            editor.size = size;
            let (width, height) = pixels(size);
            (editor.resize)(width, height);
        }
    }

    pub(crate) fn close_editor(&mut self) {
        if let Some(editor) = self.editor.take() {
            editor.view.removeFromSuperview();
        }
    }

    fn view_controller(&self) -> Result<Retained<NSViewController>, AuError> {
        let slot: Slot<Handoff<Option<Retained<AUViewControllerBase>>>> = Slot::new();
        let fill = slot.clone();
        let completion = RcBlock::new(move |controller: *mut AUViewControllerBase| {
            fill.fill(Handoff(unsafe { Retained::retain(controller) }));
        });
        {
            let engine = lock(&self.engine);
            let unit = engine.unit()?;
            unsafe { unit.requestViewControllerWithCompletionHandler(&completion) };
        }
        slot.wait().0.ok_or(AuError::NoEditor)
    }

    /// A new view from the view factory a version 2 unit names in `kAudioUnitProperty_CocoaUI`,
    /// or `None` for other units.
    fn cocoa_view(&self) -> Result<Option<Retained<NSView>>, AuError> {
        let engine = lock(&self.engine);
        let Some(bridge) = engine.unit()?.downcast_ref::<AUAudioUnitV2Bridge>() else {
            return Ok(None);
        };
        let unit = unsafe { bridge.audioUnit() };
        let mut bytes = 0u32;
        let status = unsafe {
            AudioUnitGetPropertyInfo(
                unit,
                kAudioUnitProperty_CocoaUI,
                kAudioUnitScope_Global,
                0,
                &mut bytes,
                std::ptr::null_mut(),
            )
        };
        let url = size_of::<NonNull<CFURL>>();
        let name = size_of::<NonNull<CFString>>();
        let count = (bytes as usize).saturating_sub(url) / name;
        if status != 0 || count == 0 {
            return Ok(None);
        }
        // AudioUnitCocoaViewInfo with `count` class names; pointer-aligned storage.
        let mut storage = vec![std::ptr::null_mut::<c_void>(); 1 + count];
        let status = unsafe {
            AudioUnitGetProperty(
                unit,
                kAudioUnitProperty_CocoaUI,
                kAudioUnitScope_Global,
                0,
                NonNull::new_unchecked(storage.as_mut_ptr().cast()),
                NonNull::from(&mut bytes),
            )
        };
        if status != 0 {
            return Ok(None);
        }
        let info = storage.as_ptr().cast::<AudioUnitCocoaViewInfo>();
        // SAFETY: the unit filled in the location and `count` class names, which the caller
        // owns and releases.
        let location = unsafe { CFRetained::from_raw((*info).mCocoaAUViewBundleLocation) };
        let names: Vec<CFRetained<CFString>> = storage[1..]
            .iter()
            .filter_map(|&name| NonNull::new(name.cast::<CFString>()))
            .map(|name| unsafe { CFRetained::from_raw(name) })
            .collect();
        let Some(bundle) = CFBundle::new(None, Some(&location)) else {
            return Ok(None);
        };
        // SAFETY: loads the view bundle the unit names, as any host showing its view does.
        if !unsafe { bundle.load_executable() } {
            return Ok(None);
        }
        let Some(class) = CString::new(names[0].to_string())
            .ok()
            .and_then(|name| AnyClass::get(&name))
        else {
            return Ok(None);
        };
        let factory: Retained<AnyObject> = unsafe { msg_send![class, new] };
        Ok(unsafe {
            msg_send![&factory, uiViewForAudioUnit: unit, withSize: NSSize::new(0.0, 0.0)]
        })
    }
}

fn has_area(size: NSSize) -> bool {
    size.width > 0.0 && size.height > 0.0
}

fn pixels(size: NSSize) -> (u32, u32) {
    (size.width.round() as u32, size.height.round() as u32)
}
