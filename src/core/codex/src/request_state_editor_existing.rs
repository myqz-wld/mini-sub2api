use super::ConversationAssignment;
use super::RequestStateEditor;
use super::ThreadAssignment;
use super::TurnAssignment;
use super::touch_day;

impl RequestStateEditor<'_> {
    pub(crate) fn current_turn_id(&self, thread_id: &str) -> Option<String> {
        self.scope()
            .conversations
            .values()
            .find(|entry| entry.id == thread_id)
            .and_then(|entry| entry.current_turn_id.clone())
            .or_else(|| {
                self.scope()
                    .child_threads
                    .values()
                    .find(|entry| entry.id == thread_id)
                    .and_then(|entry| entry.current_turn_id.clone())
            })
    }

    pub(crate) fn set_current_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.scope()
                .turns
                .values()
                .any(|turn| turn.id == turn_id && turn.thread_id == thread_id),
            "current turn does not belong to thread"
        );
        if let Some(entry) = self
            .scope_mut()
            .conversations
            .values_mut()
            .find(|entry| entry.id == thread_id)
        {
            self.changed |=
                super::replace_if_different(&mut entry.current_turn_id, Some(turn_id.to_string()));
            return Ok(());
        }
        if let Some(entry) = self
            .scope_mut()
            .child_threads
            .values_mut()
            .find(|entry| entry.id == thread_id)
        {
            self.changed |=
                super::replace_if_different(&mut entry.current_turn_id, Some(turn_id.to_string()));
            return Ok(());
        }
        anyhow::bail!("current turn thread is missing")
    }

    pub(crate) fn conversation_by_id(
        &mut self,
        id: &str,
    ) -> Option<(String, ConversationAssignment)> {
        let key = self
            .scope()
            .conversations
            .iter()
            .find_map(|(key, entry)| (entry.id == id).then(|| key.clone()))?;
        let day = self.day;
        let (assignment, touched) = {
            let entry = self.scope_mut().conversations.get_mut(&key)?;
            let touched = touch_day(&mut entry.last_seen_day, day);
            (
                ConversationAssignment {
                    id: entry.id.clone(),
                    window_number: entry.window_number,
                },
                touched,
            )
        };
        self.changed |= touched;
        self.protected
            .conversations
            .insert((self.scope_key.clone(), key.clone()));
        Some((key, assignment))
    }

    pub(crate) fn child_thread_by_id(&mut self, id: &str) -> Option<(String, ThreadAssignment)> {
        let key = self
            .scope()
            .child_threads
            .iter()
            .find_map(|(key, entry)| (entry.id == id).then(|| key.clone()))?;
        let assignment = self.existing_child_thread(&key)?;
        Some((key, assignment))
    }

    pub(crate) fn turn_by_id(&mut self, id: &str) -> Option<(String, TurnAssignment)> {
        let key = self
            .scope()
            .turns
            .iter()
            .find_map(|(key, entry)| (entry.id == id).then(|| key.clone()))?;
        let assignment = self.existing_turn(&key)?;
        Some((key, assignment))
    }
}
