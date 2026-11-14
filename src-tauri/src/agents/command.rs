#[derive(Debug, Eq, PartialEq)]
pub struct GuestCommand {
    program: &'static str,
    arguments: Vec<String>,
}

impl GuestCommand {
    pub(super) fn new(program: &'static str, arguments: Vec<String>) -> Self {
        Self { program, arguments }
    }

    pub fn in_guest(self, reference: String) -> Vec<String> {
        let mut arguments = vec!["sh".into(), reference, self.program.into()];
        arguments.extend(self.arguments);
        arguments
    }

    pub fn in_profile(self) -> Vec<String> {
        self.arguments
    }
}
