//! The first `ConPTY` cursor query belongs to the service's new empty terminal.
//! Consume it once so a later-attaching view cannot answer a replayed query.
//! <https://learn.microsoft.com/en-us/windows/console/createpseudoconsole>
#[derive(Default)]
pub(super) struct StartupCursor {
    pending: Vec<u8>,
    finished: bool,
}

impl StartupCursor {
    pub fn consume(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        const QUERY: &[u8] = b"\x1b[6n";
        if self.finished {
            return (bytes.to_vec(), false);
        }
        self.pending.extend_from_slice(bytes);
        if let Some(offset) = self
            .pending
            .windows(QUERY.len())
            .position(|part| part == QUERY)
        {
            self.pending.drain(offset..offset + QUERY.len());
            self.finished = true;
            return (std::mem::take(&mut self.pending), true);
        }
        let keep = (1..QUERY.len())
            .rev()
            .find(|&length| self.pending.ends_with(&QUERY[..length]))
            .unwrap_or(0);
        let visible = self.pending.len() - keep;
        (self.pending.drain(..visible).collect(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_cursor_is_answered_once_across_every_read_boundary() {
        for split in 0..=4 {
            let mut cursor = StartupCursor::default();
            let query = b"\x1b[6n";
            let (mut visible, first) = cursor.consume(&query[..split]);
            let (rest, second) = cursor.consume(&query[split..]);
            visible.extend(rest);
            assert!(visible.is_empty());
            assert_ne!(first, second);
            assert_eq!(
                cursor.consume(b"READY\x1b[6n"),
                (b"READY\x1b[6n".to_vec(), false)
            );
        }
    }

    #[test]
    fn startup_cursor_preserves_other_control_sequences_and_output() {
        let mut cursor = StartupCursor::default();
        assert_eq!(cursor.consume(b"before\x1b["), (b"before".to_vec(), false));
        assert_eq!(
            cursor.consume(b"?25l\x1b[6nREADY"),
            (b"\x1b[?25lREADY".to_vec(), true)
        );
    }
}
