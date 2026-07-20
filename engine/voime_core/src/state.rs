//! RUST-03 SessionState 状态机（对照 PRD 4.1）。
//!
//! 流转：Idle --start--> Recording --stop--> Finalizing --(Worker 收尾完)--> Idle。
//! Finalizing 期间 start 拒绝返回 -3；重复 start 返回 -1；未在录音 stop 返回 -1。

use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle = 0,
    Recording = 1,
    Finalizing = 2,
}

pub struct StateMachine {
    state: Mutex<SessionState>,
}

impl StateMachine {
    pub fn new() -> Self {
        Self { state: Mutex::new(SessionState::Idle) }
    }

    pub fn state(&self) -> SessionState {
        *self.state.lock().unwrap()
    }

    /// 0=成功, -1=已在录音, -3=Finalizing 拒绝
    pub fn start(&self) -> i32 {
        let mut s = self.state.lock().unwrap();
        match *s {
            SessionState::Idle => {
                *s = SessionState::Recording;
                0
            }
            SessionState::Recording => -1,
            SessionState::Finalizing => -3,
        }
    }

    /// 0=成功, -1=未在录音
    pub fn stop(&self) -> i32 {
        let mut s = self.state.lock().unwrap();
        if *s == SessionState::Recording {
            *s = SessionState::Finalizing;
            0
        } else {
            -1
        }
    }

    /// 仅 Worker 在收尾完成后调用：Finalizing -> Idle
    pub fn finalize_done(&self) {
        let mut s = self.state.lock().unwrap();
        if *s == SessionState::Finalizing {
            *s = SessionState::Idle;
        }
    }

    #[allow(dead_code)]
    pub fn force_idle(&self) {
        *self.state.lock().unwrap() = SessionState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions() {
        let sm = StateMachine::new();
        assert_eq!(sm.stop(), -1); // 未在录音
        assert_eq!(sm.start(), 0);
        assert_eq!(sm.start(), -1); // 重复
        assert_eq!(sm.stop(), 0);
        assert_eq!(sm.start(), -3); // Finalizing 拒绝
        sm.finalize_done();
        assert_eq!(sm.start(), 0);
    }
}
