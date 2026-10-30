//! The replay's history on disk: written as events settle, read back when the chat wakes or
//! a core takes it over, and reached further back into the agent CLI's own record.

use std::collections::HashSet;

use serde_json::{json, Value};
use sikemux_fold::{ChatEvent, ChatEventKind, PageOf, UpdateKind, TAIL_TURNS};

use super::super::super::history::{History, Recovered, MAX_HISTORY_BYTES};
use super::super::{MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, PAGE_BYTES};
use super::{stamp, Entry, Replay, PIECE_BYTES};

impl Replay {
    /// The replay of a chat an earlier core left, read back from its history
    /// file `name` in `dir`: the newest events it has room for, and what still
    /// stands from all of them.
    pub(in super::super) fn recovered(dir: &std::path::Path, name: &str) -> Self {
        let mut replay = Self::new(MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, None);
        let mut highest = 0;
        let history = History::recover(dir, name, MAX_HISTORY_BYTES, |recovered| {
            highest = highest.max(recovered.seq);
            replay.restore(recovered);
        });
        replay.history = Some(history);
        replay.seq = highest;
        replay
    }

    fn restore(&mut self, recovered: Recovered) {
        let Recovered {
            index,
            seq,
            starts_turn,
            point,
            bytes,
            event,
        } = recovered;
        let kind = event.kind;
        self.note_standing(kind, index, &event);
        self.note_subagent(kind, index, &event);
        if kind == ChatEventKind::SessionUpdate {
            self.work.note(&event.payload);
        }
        match kind {
            ChatEventKind::TurnStarted => self.in_turn = true,
            ChatEventKind::Prompt => self.awaiting = true,
            ChatEventKind::TurnCompleted | ChatEventKind::Error => {
                self.in_turn = false;
                self.awaiting = false;
            }
            _ => {}
        }
        self.last_from_user = kind == ChatEventKind::Prompt
            || UpdateKind::within(&event.payload) == Some(UpdateKind::UserMessageChunk);
        self.entries.push_back(Entry {
            event,
            bytes,
            index,
            seq,
            starts_turn,
            point,
            on_disk: true,
        });
        self.bytes += bytes;
        self.next_index = index + 1;
        self.trim();
    }

    /// Writes every event not yet on disk to the history and lets go of them in memory, for a
    /// chat that sleeps; answers how many bytes the history keeps.
    pub(in super::super) fn rest(&mut self) -> u64 {
        for entry in &mut self.entries {
            Self::write(&mut self.history, entry);
        }
        if self
            .history
            .as_ref()
            .is_some_and(|history| history.first().is_some())
        {
            self.entries.clear();
            self.bytes = 0;
        }
        self.history.as_ref().map_or(0, History::size)
    }

    /// Holds the chat's latest turns again, read from its history, once it let go of them to
    /// sleep or to stay within the core's memory and is read again: as many as a client
    /// opening the chat is shown, before any it took in since. Reads the disk, so it blocks.
    pub(in super::super) fn reload(&mut self) {
        let held_turns = self
            .entries
            .iter()
            .filter(|entry| entry.starts_turn)
            .count();
        let before = self
            .entries
            .front()
            .map_or(self.next_index, |entry| entry.index);
        let on_disk = self.history.as_ref().and_then(History::first);
        if held_turns >= TAIL_TURNS
            || self.bytes as u64 >= PAGE_BYTES
            || on_disk.is_none_or(|first| first >= before)
        {
            return;
        }
        let read = self
            .plan(PageOf::Before(before), TAIL_TURNS - held_turns)
            .and_then(|planned| match &planned.reader {
                Some(reader) => reader
                    .lines(planned.lo, planned.hi.min(before))
                    .map_err(|error| error.to_string()),
                None => Ok(Vec::new()),
            });
        let lines = match read {
            Ok(lines) => lines,
            Err(error) => {
                eprintln!("sikemux core: a chat's history could not be read again: {error}");
                return;
            }
        };
        for line in lines.into_iter().rev() {
            self.bytes += line.bytes;
            self.entries.push_front(Entry {
                event: line.event,
                bytes: line.bytes,
                index: line.index,
                seq: line.seq,
                starts_turn: line.starts_turn,
                point: line.point,
                on_disk: true,
            });
        }
        self.trim();
    }

    /// Puts `updates` before everything the chat holds, in its history on disk, answering
    /// whether they went in. They are older than what stands or is still going.
    pub(in super::super) fn prepend(&mut self, updates: Vec<(String, Value)>) -> bool {
        let Some(first) = self.history.as_ref().and_then(History::first) else {
            return false;
        };
        let Some(base) = first.checked_sub(updates.len() as u64) else {
            return false;
        };
        let mut events = Vec::with_capacity(updates.len());
        let mut known: HashSet<String> = self
            .spawns
            .iter()
            .map(|spawn| spawn.subagent.clone())
            .collect();
        let mut added: HashSet<String> = HashSet::new();
        let (mut last_from_user, mut since_point) = (false, 0);
        for (offset, (session, update)) in updates.into_iter().enumerate() {
            let index = base + offset as u64;
            let mut payload = json!({ "sessionId": session, "update": update });
            stamp(&mut payload);
            let from_user = UpdateKind::within(&payload) == Some(UpdateKind::UserMessageChunk);
            let starts_turn = from_user && !last_from_user;
            last_from_user = from_user;
            let point = starts_turn || since_point >= PIECE_BYTES;
            if point {
                since_point = 0;
            }
            since_point += serde_json::to_vec(&payload).map_or(0, |bytes| bytes.len());
            let event = ChatEvent {
                kind: ChatEventKind::SessionUpdate,
                payload,
            };
            let subagent = event
                .payload
                .pointer("/update/subagentSessionId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match (UpdateKind::within(&event.payload), subagent) {
                (Some(UpdateKind::SubagentSpawned), Some(subagent))
                    if !known.contains(&subagent) =>
                {
                    self.note_subagent(ChatEventKind::SessionUpdate, index, &event);
                    known.insert(subagent.clone());
                    added.insert(subagent);
                }
                (Some(UpdateKind::SubagentStateUpdate), Some(subagent))
                    if added.contains(&subagent) =>
                {
                    self.note_subagent(ChatEventKind::SessionUpdate, index, &event);
                }
                _ => {}
            }
            events.push((index, event, point, starts_turn));
        }
        let seq = self.numbered();
        let Some(history) = self.history.as_mut() else {
            return false;
        };
        history.prepend(&events, seq);
        history.first().is_some_and(|now| now < first)
    }

    /// Writes the newest event to the history, which nothing joins after.
    pub(in super::super) fn settle(&mut self) {
        if let Some(last) = self.entries.back_mut() {
            Self::write(&mut self.history, last);
        }
    }

    pub(super) fn write(history: &mut Option<History>, entry: &mut Entry) {
        if entry.on_disk {
            return;
        }
        entry.on_disk = true;
        if let Some(history) = history.as_mut() {
            history.append(
                entry.index,
                entry.seq,
                &entry.event,
                entry.point,
                entry.starts_turn,
            );
        }
    }
}
