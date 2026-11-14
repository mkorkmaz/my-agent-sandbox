use super::{volume_uuid, Project};

#[derive(Debug, Eq, PartialEq)]
pub enum FolderState {
    Available,
    Unavailable,
    Replaced,
}

impl Project {
    pub fn folder_state(&self) -> FolderState {
        let Ok(metadata) = self.path.metadata() else {
            return FolderState::Unavailable;
        };
        if self.matches_folder(&metadata, volume_uuid(&self.path), false) {
            return FolderState::Available;
        }
        FolderState::Replaced
    }
}
