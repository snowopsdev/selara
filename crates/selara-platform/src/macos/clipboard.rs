use anyhow::{Context, Result};
use arboard::Clipboard;
use async_trait::async_trait;
use std::sync::Mutex;

use crate::ClipboardService;

pub struct MacosClipboard {
    inner: Mutex<Clipboard>,
}

impl MacosClipboard {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(Clipboard::new().context("open clipboard")?),
        })
    }
}

#[async_trait]
impl ClipboardService for MacosClipboard {
    async fn get_text(&self) -> Result<Option<String>> {
        let mut clip = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match clip.get_text() {
            Ok(t) => Ok(Some(t)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn set_text(&self, text: &str) -> Result<()> {
        let mut clip = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        clip.set_text(text.to_string())?;
        Ok(())
    }
}
