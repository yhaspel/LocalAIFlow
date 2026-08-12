//! Minimal, hand-written FFI to the macOS Accessibility (AX) C API.
//!
//! These signatures are the stable, decades-old ApplicationServices ABI
//! (AXUIElement.h). We bind exactly what the inserter/selection code needs
//! rather than pulling an unmaintained wrapper crate.

#![allow(non_upper_case_globals)]

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use std::os::raw::c_void;

pub type AXUIElementRef = *const c_void;
pub type AXError = i32;

pub const kAXErrorSuccess: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut u8, // Boolean
    ) -> AXError;
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> u8;
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// Owning guard for CF objects returned under the Create/Copy rule.
pub struct OwnedCF(pub CFTypeRef);

impl Drop for OwnedCF {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

pub fn attr(name: &'static str) -> CFString {
    CFString::from_static_string(name)
}

/// Copy an attribute; returns an owning guard on success.
pub fn copy_attr(element: AXUIElementRef, name: &'static str) -> Result<OwnedCF, AXError> {
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, attr(name).as_concrete_TypeRef(), &mut out)
    };
    if err == kAXErrorSuccess && !out.is_null() {
        Ok(OwnedCF(out))
    } else {
        Err(err)
    }
}

/// Interpret a copied attribute value as a string, if it is one.
pub fn cf_as_string(value: &OwnedCF) -> Option<String> {
    unsafe {
        if core_foundation::base::CFGetTypeID(value.0) == CFString::type_id() {
            let s = CFString::wrap_under_get_rule(value.0 as CFStringRef);
            Some(s.to_string())
        } else {
            None
        }
    }
}

pub fn set_string_attr(
    element: AXUIElementRef,
    name: &'static str,
    value: &str,
) -> Result<(), AXError> {
    let cf = CFString::new(value);
    let err = unsafe {
        AXUIElementSetAttributeValue(
            element,
            attr(name).as_concrete_TypeRef(),
            cf.as_concrete_TypeRef() as CFTypeRef,
        )
    };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(err)
    }
}

pub fn attr_settable(element: AXUIElementRef, name: &'static str) -> bool {
    let mut settable: u8 = 0;
    let err = unsafe {
        AXUIElementIsAttributeSettable(element, attr(name).as_concrete_TypeRef(), &mut settable)
    };
    err == kAXErrorSuccess && settable != 0
}

/// The focused UI element of the frontmost app, via the system-wide element.
/// Requires Accessibility permission.
pub fn focused_element() -> Result<OwnedCF, String> {
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Err("Accessibility permission not granted".into());
        }
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return Err("AXUIElementCreateSystemWide returned null".into());
        }
        let system_guard = OwnedCF(system as CFTypeRef);
        let focused = copy_attr(system_guard.0 as AXUIElementRef, "AXFocusedUIElement")
            .map_err(|e| format!("no focused UI element (AXError {e})"))?;
        Ok(focused)
    }
}
