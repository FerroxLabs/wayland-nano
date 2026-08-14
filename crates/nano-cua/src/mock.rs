//! Test-support backend: a scripted, in-memory [`ComputerUseBackend`] used
//! by the integrator's wiring tests (journal ordering, cancel race, panic
//! containment, focus-loss). NOT a production backend — it performs no OS
//! input injection; it records every dispatched op for assertions.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::CuaResult;
use crate::backend::{ComputerUseBackend, Platform};
use crate::error::CuaError;
use crate::op::{CuaOp, CuaOpResult};

/// One scripted dispatch behavior, consumed in FIFO order; when the script
/// is empty the mock completes every op (screenshots return a 1x1 PNG).
#[derive(Debug)]
pub enum MockBehavior {
    /// Fail the dispatch with this typed error.
    Fail(CuaError),
    /// Panic inside dispatch (the panic-containment proof: the integrator
    /// must surface a typed error, never a process abort).
    Panic,
    /// Never return (the cancel-race / kill-mid-dispatch proof: the task is
    /// aborted or the turn dropped while this dispatch is in flight).
    Hang,
}

/// A 1x1 transparent PNG, produced through the workspace-pinned `image`
/// codec (never hand-rolled bytes).
fn tiny_png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("1x1 png encodes");
    buf.into_inner()
}

#[derive(Debug, Default)]
struct MockState {
    frontmost: Option<String>,
    script: VecDeque<MockBehavior>,
    dispatched: Vec<CuaOp>,
}

/// The scripted backend. Clone-cheap via Arc internals; tests share a handle
/// with the integrator's session wrapper to set focus, push behaviors, and
/// inspect what was dispatched.
#[derive(Clone, Default)]
pub struct MockBackend {
    state: Arc<Mutex<MockState>>,
    /// Runs at dispatch ENTRY (before the scripted behavior) — the cancel
    /// race test sets the cancel flag here, the kill test uses it to learn
    /// the dispatch is in flight.
    on_dispatch: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for MockBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockBackend").finish_non_exhaustive()
    }
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_frontmost(self, app: impl Into<String>) -> Self {
        self.state.lock().expect("mock state").frontmost = Some(app.into());
        self
    }

    pub fn with_on_dispatch(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.on_dispatch = Some(hook);
        self
    }

    /// Change the frontmost app mid-test (the §5.1 focus-loss proof: a
    /// dispatch approved against app A fails `FocusLost` when the mock's
    /// frontmost moved to B).
    pub fn set_frontmost(&self, app: Option<String>) {
        self.state.lock().expect("mock state").frontmost = app;
    }

    pub fn push_behavior(&self, behavior: MockBehavior) {
        self.state
            .lock()
            .expect("mock state")
            .script
            .push_back(behavior);
    }

    /// Every op dispatched so far, in order (the journal-before-dispatch
    /// assertion inspects this).
    pub fn dispatched(&self) -> Vec<CuaOp> {
        self.state.lock().expect("mock state").dispatched.clone()
    }
}

#[async_trait::async_trait]
impl ComputerUseBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn platform(&self) -> Platform {
        Platform::Unsupported
    }

    async fn dispatch(
        &self,
        expected_frontmost_app: Option<&str>,
        op: CuaOp,
    ) -> CuaResult<CuaOpResult> {
        if let Some(hook) = &self.on_dispatch {
            hook();
        }
        let behavior = {
            let mut state = self.state.lock().expect("mock state");
            state.dispatched.push(op.clone());
            state.script.pop_front()
        };
        match behavior {
            Some(MockBehavior::Fail(err)) => return Err(err),
            Some(MockBehavior::Panic) => panic!("mock backend panic (scripted)"),
            Some(MockBehavior::Hang) => std::future::pending::<()>().await,
            None => {}
        }
        // The §5.1 contract: re-resolve the frontmost app immediately before
        // dispatch and compare against the approved value.
        if let Some(expected) = expected_frontmost_app {
            let current = self.state.lock().expect("mock state").frontmost.clone();
            if current.as_deref() != Some(expected) {
                return Err(CuaError::FocusLost);
            }
        }
        Ok(match op {
            CuaOp::Screenshot { format, .. } => {
                use base64::Engine as _;
                CuaOpResult::Screenshot {
                    format,
                    data_b64: base64::engine::general_purpose::STANDARD.encode(tiny_png()),
                    width: 1,
                    height: 1,
                    redacted: false,
                }
            }
            CuaOp::FrontmostApp {} => CuaOpResult::FrontmostApp {
                app_id: self.state.lock().expect("mock state").frontmost.clone(),
            },
            _ => CuaOpResult::Ok,
        })
    }

    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(self.state.lock().expect("mock state").frontmost.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MouseButton, Region, ScreenshotFormat};

    #[tokio::test(flavor = "current_thread")]
    async fn mock_records_and_completes_by_default() {
        let backend = MockBackend::new().with_frontmost("notepad.exe");
        assert_eq!(
            backend.frontmost_app().await.unwrap(),
            Some("notepad.exe".to_string())
        );
        let result = backend
            .dispatch(
                Some("notepad.exe"),
                CuaOp::LeftClick {
                    x: 1,
                    y: 2,
                    button: MouseButton::Left,
                    mods: Default::default(),
                },
            )
            .await
            .unwrap();
        assert_eq!(result, CuaOpResult::Ok);
        assert_eq!(backend.dispatched().len(), 1);
        let shot = backend
            .dispatch(
                None,
                CuaOp::Screenshot {
                    region: Region::Full,
                    format: ScreenshotFormat::Png,
                    redact: true,
                },
            )
            .await
            .unwrap();
        let CuaOpResult::Screenshot { data_b64, .. } = shot else {
            panic!("screenshot result")
        };
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mock_scripts_failures_and_focus_loss() {
        let backend = MockBackend::new().with_frontmost("a.exe");
        backend.push_behavior(MockBehavior::Fail(CuaError::Backend));
        let err = backend.dispatch(None, CuaOp::Wait { duration_ms: 1 }).await;
        assert!(matches!(err, Err(CuaError::Backend)));
        // Focus moved since approval: typed FocusLost, not dispatched.
        backend.set_frontmost(Some("b.exe".into()));
        let err = backend
            .dispatch(Some("a.exe"), CuaOp::Wait { duration_ms: 1 })
            .await;
        assert!(matches!(err, Err(CuaError::FocusLost)));
    }
}
