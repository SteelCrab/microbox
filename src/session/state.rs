use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Created,
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
}

impl SessionState {
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        let valid = matches!(
            (self, next),
            (Self::Created, Self::Starting)
                | (Self::Created, Self::Failed)
                | (Self::Starting, Self::Running)
                | (Self::Starting, Self::Failed)
                | (Self::Running, Self::Stopping)
                | (Self::Running, Self::Exited)
                | (Self::Running, Self::Failed)
                | (Self::Stopping, Self::Exited)
                | (Self::Stopping, Self::Failed)
        );
        if valid {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub from: SessionState,
    pub to: SessionState,
}

impl Display for TransitionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid session transition: {:?} -> {:?}",
            self.from, self.to
        )
    }
}

impl Error for TransitionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_lifecycle() {
        let state = SessionState::Created
            .transition(SessionState::Starting)
            .unwrap()
            .transition(SessionState::Running)
            .unwrap()
            .transition(SessionState::Stopping)
            .unwrap()
            .transition(SessionState::Exited)
            .unwrap();
        assert_eq!(state, SessionState::Exited);
    }

    #[test]
    fn terminal_states_cannot_restart() {
        assert!(
            SessionState::Exited
                .transition(SessionState::Starting)
                .is_err()
        );
    }
}
