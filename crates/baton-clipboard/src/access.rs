use baton_core::ports::{ClipboardAccess, ClipboardContent, ClipboardRead};
use baton_core::{Error, Result};
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData};

const SENSITIVE_FORMATS: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "ExcludeClipboardContentFromMonitorProcessing",
];

/// 本机系统剪贴板。每次读写新建上下文，不跨线程长期持有。
pub struct SystemClipboard;

impl SystemClipboard {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

fn open() -> Result<ClipboardContext> {
    ClipboardContext::new().map_err(|e| Error::Clipboard(e.to_string()))
}

fn is_sensitive(formats: &[String]) -> bool {
    formats.iter().any(|found| {
        SENSITIVE_FORMATS
            .iter()
            .any(|known| found.eq_ignore_ascii_case(known))
    })
}

impl ClipboardAccess for SystemClipboard {
    fn read(&self) -> Result<ClipboardRead> {
        let ctx = open()?;
        let formats = ctx
            .available_formats()
            .map_err(|e| Error::Clipboard(e.to_string()))?;
        if is_sensitive(&formats) {
            return Ok(ClipboardRead::Sensitive);
        }
        if let Ok(image) = ctx.get_image()
            && !image.is_empty()
        {
            let png = image
                .to_png()
                .map_err(|e| Error::Clipboard(e.to_string()))?;
            let bytes = png.get_bytes().to_vec();
            if !bytes.is_empty() {
                return Ok(ClipboardRead::Content(ClipboardContent::ImagePng(bytes)));
            }
        }
        if let Ok(text) = ctx.get_text() {
            let text = text.trim_end_matches('\0').to_owned();
            if !text.is_empty() {
                return Ok(ClipboardRead::Content(ClipboardContent::Text(text)));
            }
        }
        Ok(ClipboardRead::Empty)
    }

    fn write(&self, content: &ClipboardContent) -> Result<()> {
        let ctx = open()?;
        match content {
            ClipboardContent::Text(text) => ctx
                .set_text(text.clone())
                .map_err(|e| Error::Clipboard(e.to_string())),
            ClipboardContent::ImagePng(bytes) => {
                let image = RustImageData::from_bytes(bytes)
                    .map_err(|e| Error::Clipboard(e.to_string()))?;
                ctx.set_image(image)
                    .map_err(|e| Error::Clipboard(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_sensitive_type_names() {
        assert!(is_sensitive(&["org.nspasteboard.ConcealedType".into()]));
        assert!(is_sensitive(&[
            "ExcludeClipboardContentFromMonitorProcessing".into()
        ]));
        assert!(is_sensitive(&[
            "public.utf8-plain-text".into(),
            "excludeclipboardcontentfrommonitorprocessing".into(),
        ]));
        assert!(!is_sensitive(&["public.utf8-plain-text".into()]));
    }
}
