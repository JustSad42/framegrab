//! Drag a saved capture out of the toolbar card into Discord, a browser, Explorer…

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName};
use windows_core::{BOOL, HRESULT, HSTRING, implement};

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Runs a modal drag of the file. Returns true if it was dropped somewhere.
pub fn drag_file(path: &std::path::Path) -> bool {
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path.as_os_str()), None) else { return false };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return false };
        let src: IDropSource = Source.into();
        let mut effect = DROPEFFECT(0);
        let hr = DoDragDrop(&data, &src, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
        hr == DRAGDROP_S_DROP && effect.0 != 0
    }
}
