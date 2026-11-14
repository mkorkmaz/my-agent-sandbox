use std::sync::Mutex;

#[derive(Default)]
enum SurfaceState {
    #[default]
    Reserved,
    Attached(usize),
    Closed,
}

#[derive(Default)]
pub(super) struct Surface(Mutex<SurfaceState>);

impl Surface {
    pub fn attach(&self, pointer: usize) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|error| error.to_string())?;
        if pointer == 0 || !matches!(*state, SurfaceState::Reserved) {
            return Err("The terminal view is no longer reserved.".into());
        }
        *state = SurfaceState::Attached(pointer);
        Ok(())
    }

    pub fn pointer(&self) -> usize {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        match *state {
            SurfaceState::Attached(pointer) => pointer,
            _ => 0,
        }
    }

    pub fn close(&self) -> usize {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        match std::mem::replace(&mut *state, SurfaceState::Closed) {
            SurfaceState::Attached(pointer) => pointer,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_reservation_blocks_late_native_view_publication() {
        let surface = Surface::default();
        assert_eq!(surface.close(), 0);
        assert!(surface.attach(1).is_err());
        assert_eq!(surface.pointer(), 0);
        let attached = Surface::default();
        attached.attach(1).unwrap();
        assert!(attached.attach(2).is_err());
        assert_eq!(attached.close(), 1);
        assert_eq!(attached.close(), 0);
        assert!(attached.attach(1).is_err());
    }
}
