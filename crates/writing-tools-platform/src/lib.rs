//! OS integration surface. Desktop shells implement these traits per platform.
//! The first spike ships a no-op / stdin-stdout backend so core can be tested
//! without Accessibility permissions.

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct SelectionSnapshot {
    pub text: String,
    pub app_name: Option<String>,
}

#[async_trait]
pub trait SelectionService: Send + Sync {
    async fn read_selection(&self) -> Result<Option<SelectionSnapshot>>;
    async fn replace_selection(&self, text: &str) -> Result<()>;
}

#[async_trait]
pub trait HotkeyService: Send + Sync {
    /// Register the configured hotkey. Implementations should call `on_fire` on the UI/runtime thread.
    async fn register(&self, hotkey: &str, on_fire: Box<dyn Fn() + Send + Sync>) -> Result<()>;
}

#[async_trait]
pub trait ClipboardService: Send + Sync {
    async fn get_text(&self) -> Result<Option<String>>;
    async fn set_text(&self, text: &str) -> Result<()>;
}

/// Dev/testing backend: no real OS hooks.
pub struct NullPlatform;

#[async_trait]
impl SelectionService for NullPlatform {
    async fn read_selection(&self) -> Result<Option<SelectionSnapshot>> {
        Ok(None)
    }

    async fn replace_selection(&self, _text: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl HotkeyService for NullPlatform {
    async fn register(&self, _hotkey: &str, _on_fire: Box<dyn Fn() + Send + Sync>) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl ClipboardService for NullPlatform {
    async fn get_text(&self) -> Result<Option<String>> {
        Ok(None)
    }

    async fn set_text(&self, _text: &str) -> Result<()> {
        Ok(())
    }
}
