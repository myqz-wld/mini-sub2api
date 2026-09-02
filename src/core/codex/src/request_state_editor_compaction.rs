use super::*;
use crate::request_state_types::CompactionMarkerEntry;
use crate::request_state_types::validate_lookup_key;

impl<'a> RequestStateEditor<'a> {
    pub(crate) fn window_number(&self, thread_id: &str) -> Option<u64> {
        let scope = self.scope();
        scope
            .conversations
            .values()
            .find(|entry| entry.id == thread_id)
            .map(|entry| entry.window_number)
            .or_else(|| {
                scope
                    .child_threads
                    .values()
                    .find(|entry| entry.id == thread_id)
                    .map(|entry| entry.window_number)
            })
    }

    pub(crate) fn set_window_number(&mut self, thread_id: &str, window: u64) -> Result<()> {
        let scope = self.scope_mut();
        if let Some(entry) = scope
            .conversations
            .values_mut()
            .find(|entry| entry.id == thread_id)
        {
            self.changed |= replace_if_different(&mut entry.window_number, window);
            return Ok(());
        }
        if let Some(entry) = scope
            .child_threads
            .values_mut()
            .find(|entry| entry.id == thread_id)
        {
            self.changed |= replace_if_different(&mut entry.window_number, window);
            return Ok(());
        }
        anyhow::bail!("window thread is not in request state")
    }

    pub(crate) fn observe_window_number(&mut self, thread_id: &str, window: u64) -> Result<u64> {
        let current = self
            .window_number(thread_id)
            .ok_or_else(|| anyhow::anyhow!("window thread is not in request state"))?;
        if window > current {
            self.set_window_number(thread_id, window)?;
            Ok(window)
        } else {
            Ok(current)
        }
    }

    pub(crate) fn begin_compaction(&mut self, key: &str, thread_id: &str) -> Result<u64> {
        validate_lookup_key(key)?;
        if let Some(existing) = self.scope().compaction_markers.get(key) {
            anyhow::ensure!(
                existing.thread_id == thread_id,
                "compaction relationship changed"
            );
            let window_number = existing.window_number;
            let day = self.day;
            let scope_key = self.scope_key.clone();
            let touched = touch_day(
                &mut self
                    .scope_mut()
                    .compaction_markers
                    .get_mut(key)
                    .expect("existing compaction marker")
                    .last_seen_day,
                day,
            );
            self.changed |= touched;
            self.protected
                .compaction_markers
                .insert((scope_key, key.to_string()));
            return Ok(window_number);
        }
        let window = self
            .window_number(thread_id)
            .ok_or_else(|| anyhow::anyhow!("compaction thread is not in request state"))?
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("window number exhausted"))?;
        let entry = CompactionMarkerEntry {
            thread_id: thread_id.to_string(),
            window_number: window,
            last_seen_day: self.day,
        };
        self.scope_mut()
            .compaction_markers
            .insert(key.to_string(), entry);
        self.changed = true;
        self.protected
            .compaction_markers
            .insert((self.scope_key.clone(), key.to_string()));
        Ok(window)
    }

    pub(crate) fn commit_compaction(
        &mut self,
        key: &str,
        thread_id: &str,
        target_window: u64,
    ) -> Result<u64> {
        validate_lookup_key(key)?;
        let existing = self
            .scope()
            .compaction_markers
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("compaction marker is missing"))?;
        anyhow::ensure!(
            existing.thread_id == thread_id && existing.window_number == target_window,
            "compaction relationship changed"
        );
        let current = self
            .window_number(thread_id)
            .ok_or_else(|| anyhow::anyhow!("compaction thread is not in request state"))?;
        anyhow::ensure!(
            target_window <= current.saturating_add(1),
            "compaction target skipped a committed window"
        );
        if target_window > current {
            self.set_window_number(thread_id, target_window)?;
        }
        let day = self.day;
        let touched = touch_day(
            &mut self
                .scope_mut()
                .compaction_markers
                .get_mut(key)
                .expect("validated compaction marker")
                .last_seen_day,
            day,
        );
        self.changed |= touched;
        self.protected
            .compaction_markers
            .insert((self.scope_key.clone(), key.to_string()));
        Ok(self
            .window_number(thread_id)
            .expect("validated compaction thread"))
    }
}
