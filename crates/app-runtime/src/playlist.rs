//! What the DS is playing, and what it will play next.
//!
//! The point of holding a queue at all is that pressing Next should not blank
//! the screen while a round trip decides what happens. If we know the queue and
//! where we are in it, we already know what Next means, and can show it at once.
//!
//! Every wire format here was captured from a Selekt DSM rather than derived:
//!
//! | Action | Response |
//! | --- | --- |
//! | `Ds/Playlist:Id` | `"657"` — plain decimal |
//! | `Ds/Playlist:IdArray` | `"451" "AAACiw..."` — a token, then base64 of big-endian `u32`s |
//! | `Ds/Playlist:ReadList "659"` | `<TrackList><Entry><Id/><Uri/><Metadata/></Entry></TrackList>` |
//!
//! `Id`, `IdArray`, `Repeat` and `Shuffle` also arrive together as subscription
//! event variables, so the queue stays fresh without polling for it.

use alloc::vec::Vec as AllocVec;

use app_core::{HIFI_TEXT_LEN, HIFI_URI_LEN, HifiStatus};

/// Ceiling on queue ids held at once.
///
/// The DS publishes its own limit as `TracksMax`, observed as 1000, so this sits
/// comfortably above anything it can send. At 4 bytes an id even a full queue is
/// 4 KB; the cap is a backstop against a malformed payload, not a budget.
pub const MAX_QUEUE_TRACKS: usize = 2048;

/// How many tracks keep decoded metadata alongside the queue.
///
/// Only the immediate neighbours are ever predicted, so this needs to cover
/// current, next and previous with a little slack for a queue that moves under
/// us.
pub const TRACK_CACHE_LEN: usize = 4;

/// How long a prediction is believed before the device's own account wins.
///
/// Long enough to cover a slow skip, short enough that a wrong guess is a blip
/// rather than a stuck screen.
pub const PREDICTION_TTL_MS: u64 = 4_000;

/// The track fields a prediction can fill in ahead of the device.
///
/// Deliberately not a whole [`HifiStatus`]: elapsed time, playback state and
/// volume are not ours to guess.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackMetadata {
    pub title: heapless::String<HIFI_TEXT_LEN>,
    pub artist: heapless::String<HIFI_TEXT_LEN>,
    pub album: heapless::String<HIFI_TEXT_LEN>,
    pub album_art_uri: heapless::String<HIFI_URI_LEN>,
    pub duration_seconds: u32,
}

impl TrackMetadata {
    /// Lifts the track fields out of a status, leaving the rest behind.
    pub fn from_status(status: &HifiStatus) -> Self {
        Self {
            title: status.title.clone(),
            artist: status.artist.clone(),
            album: status.album.clone(),
            album_art_uri: status.album_art_uri.clone(),
            duration_seconds: status.duration_seconds,
        }
    }

    /// Writes the track fields onto a status, resetting the ones that describe
    /// progress through a track we are no longer playing.
    pub fn apply_to(&self, status: &mut HifiStatus) {
        status.title = self.title.clone();
        status.artist = self.artist.clone();
        status.album = self.album.clone();
        status.album_art_uri = self.album_art_uri.clone();
        status.duration_seconds = self.duration_seconds;
        status.elapsed_seconds = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.artist.is_empty() && self.album.is_empty()
    }
}

/// Which way a skip moves through the queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Forward,
    Backward,
}

/// What to do with a prediction when the device finally reports an id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reconcile {
    /// No prediction outstanding — nothing to hold back.
    Idle,
    /// The device agrees. The prediction has served its purpose.
    Confirmed,
    /// The device says something else and we still trust the prediction.
    /// Callers should keep showing what they showed.
    Holding,
    /// The prediction ran out of time. Whatever the device says is now true.
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Prediction {
    target_id: u32,
    expires_at_ms: u64,
}

/// The queue, where we are in it, and any outstanding guess about where we are
/// heading.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlaylistState {
    ids: AllocVec<u32>,
    current_id: Option<u32>,
    /// `Repeat` from the DS. With repeat on, the queue wraps, so the ends stop
    /// being dead ends.
    repeat: bool,
    /// `Shuffle` from the DS. With shuffle on, the next track is the DS's
    /// business and not a function of queue order, so we do not guess.
    shuffle: bool,
    cache: AllocVec<(u32, TrackMetadata)>,
    prediction: Option<Prediction>,
}

impl PlaylistState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the queue. Ids beyond [`MAX_QUEUE_TRACKS`] are dropped.
    pub fn set_ids(&mut self, ids: &[u32]) -> bool {
        let take = ids.len().min(MAX_QUEUE_TRACKS);
        if self.ids.len() == take && self.ids == ids[..take] {
            return false;
        }
        self.ids.clear();
        self.ids.extend_from_slice(&ids[..take]);
        // Metadata is keyed by id, so it survives a reorder. Entries for ids
        // that left the queue are evicted by the cache's own size limit.
        true
    }

    pub fn set_current_id(&mut self, id: u32) -> bool {
        if self.current_id == Some(id) {
            return false;
        }
        self.current_id = Some(id);
        true
    }

    pub fn set_repeat(&mut self, repeat: bool) {
        self.repeat = repeat;
    }

    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
    }

    pub fn current_id(&self) -> Option<u32> {
        self.current_id
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn current_index(&self) -> Option<usize> {
        let current = self.current_id?;
        self.ids.iter().position(|&id| id == current)
    }

    /// The id one step away, or `None` when we cannot say.
    ///
    /// Returns `None` under shuffle: the DS picks the next track itself, and a
    /// confident wrong answer is worse than no answer.
    pub fn neighbour_id(&self, step: Step) -> Option<u32> {
        if self.shuffle {
            return None;
        }
        let index = self.current_index()?;
        let len = self.ids.len();
        let target = match step {
            Step::Forward if index + 1 < len => index + 1,
            Step::Forward if self.repeat && len > 0 => 0,
            Step::Forward => return None,
            Step::Backward if index > 0 => index - 1,
            Step::Backward if self.repeat && len > 0 => len - 1,
            Step::Backward => return None,
        };
        self.ids.get(target).copied()
    }

    /// Ids worth holding metadata for: the neighbours we might have to show at
    /// a moment's notice, minus what is already cached.
    pub fn ids_needing_metadata(&self) -> AllocVec<u32> {
        let mut wanted = AllocVec::new();
        for step in [Step::Forward, Step::Backward] {
            if let Some(id) = self.neighbour_id(step)
                && self.cached(id).is_none()
                && !wanted.contains(&id)
            {
                wanted.push(id);
            }
        }
        wanted
    }

    pub fn cached(&self, id: u32) -> Option<&TrackMetadata> {
        self.cache
            .iter()
            .find(|(cached, _)| *cached == id)
            .map(|(_, metadata)| metadata)
    }

    /// Stores metadata for a track, evicting the oldest entry when full.
    pub fn cache_track(&mut self, id: u32, metadata: TrackMetadata) {
        if let Some(slot) = self.cache.iter_mut().find(|(cached, _)| *cached == id) {
            slot.1 = metadata;
            return;
        }
        if self.cache.len() >= TRACK_CACHE_LEN {
            self.cache.remove(0);
        }
        self.cache.push((id, metadata));
    }

    /// Takes the optimistic move for a skip, if we can make one.
    ///
    /// Returns the metadata to show right now. Also records the guess so
    /// [`reconcile`](Self::reconcile) can decide later whether it held up.
    pub fn predict(&mut self, step: Step, now_ms: u64) -> Option<TrackMetadata> {
        let target_id = self.neighbour_id(step)?;
        let metadata = self.cached(target_id)?.clone();
        if metadata.is_empty() {
            return None;
        }
        self.current_id = Some(target_id);
        self.prediction = Some(Prediction {
            target_id,
            expires_at_ms: now_ms.saturating_add(PREDICTION_TTL_MS),
        });
        Some(metadata)
    }

    /// Weighs an id reported by the device against any outstanding prediction.
    ///
    /// Mirrors Louie's `reconcileOptimistic`: confirm and clear, expire and
    /// clear, or keep holding.
    pub fn reconcile(&mut self, incoming_id: u32, now_ms: u64) -> Reconcile {
        let Some(prediction) = self.prediction else {
            return Reconcile::Idle;
        };
        if incoming_id == prediction.target_id {
            self.prediction = None;
            return Reconcile::Confirmed;
        }
        if now_ms >= prediction.expires_at_ms {
            self.prediction = None;
            return Reconcile::Expired;
        }
        Reconcile::Holding
    }

    pub fn has_prediction(&self) -> bool {
        self.prediction.is_some()
    }

    /// Abandons any outstanding prediction, for when the session drops and its
    /// state can no longer be trusted.
    pub fn forget_prediction(&mut self) {
        self.prediction = None;
    }
}

/// Decodes a `Ds/Playlist:IdArray` payload into track ids.
///
/// The DS sends base64 of big-endian `u32`s — captured, not assumed. A short
/// tail that is not a whole id is ignored rather than treated as a failure, so
/// a truncated line degrades to a shorter queue instead of no queue.
pub fn decode_id_array(encoded: &str) -> Option<AllocVec<u32>> {
    let bytes = decode_base64(encoded)?;
    let mut ids = AllocVec::new();
    for chunk in bytes.chunks_exact(4) {
        if ids.len() >= MAX_QUEUE_TRACKS {
            break;
        }
        ids.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(ids)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_base64(encoded: &str) -> Option<AllocVec<u8>> {
    let mut out = AllocVec::new();
    let mut accumulator: u32 = 0;
    let mut bits = 0_u32;
    for &byte in encoded.as_bytes() {
        if byte == b'=' {
            break;
        }
        // Whitespace is not skipped: LPEC puts the whole payload on one line,
        // so a space means this is not the base64 we were promised. Being
        // strict here is what lets a decimal list be rejected rather than
        // silently decoded into a queue of nonsense.
        let value = base64_value(byte)?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(title: &str) -> TrackMetadata {
        let mut track = TrackMetadata::default();
        let _ = track.title.push_str(title);
        track
    }

    fn queue_of(ids: &[u32], current: u32) -> PlaylistState {
        let mut state = PlaylistState::new();
        state.set_ids(ids);
        state.set_current_id(current);
        state
    }

    #[test]
    fn decodes_the_id_array_the_device_actually_sends() {
        // First 32 bytes of a real `Ds/Playlist:IdArray` response from a Selekt
        // DSM. The queue ran 651, 653, 655, ... in steps of two.
        let ids = decode_id_array("AAACiwAAAo0AAAKPAAACkQAAApMAAAKVAAAClwAAApk=").unwrap();
        assert_eq!(ids, [651, 653, 655, 657, 659, 661, 663, 665]);
    }

    #[test]
    fn a_partial_id_is_dropped_rather_than_failing_the_queue() {
        // Six bytes: one whole id and a two-byte tail.
        let ids = decode_id_array("AAACiwAA").unwrap();
        assert_eq!(ids, [651]);
    }

    #[test]
    fn rejects_payloads_that_are_not_base64() {
        assert!(decode_id_array("651 653 655").is_none());
    }

    #[test]
    fn finds_the_neighbours_of_the_current_track() {
        let state = queue_of(&[651, 653, 655, 657], 655);
        assert_eq!(state.neighbour_id(Step::Forward), Some(657));
        assert_eq!(state.neighbour_id(Step::Backward), Some(653));
    }

    #[test]
    fn the_ends_of_the_queue_are_dead_ends_without_repeat() {
        let state = queue_of(&[651, 653], 653);
        assert_eq!(state.neighbour_id(Step::Forward), None);
        assert_eq!(state.neighbour_id(Step::Backward), Some(651));
    }

    #[test]
    fn repeat_wraps_both_ends() {
        let mut state = queue_of(&[651, 653], 653);
        state.set_repeat(true);
        assert_eq!(state.neighbour_id(Step::Forward), Some(651));
        state.set_current_id(651);
        assert_eq!(state.neighbour_id(Step::Backward), Some(653));
    }

    #[test]
    fn shuffle_stops_us_guessing() {
        let mut state = queue_of(&[651, 653, 655], 653);
        assert_eq!(state.neighbour_id(Step::Forward), Some(655));
        state.set_shuffle(true);
        // The DS picks under shuffle, so queue order says nothing.
        assert_eq!(state.neighbour_id(Step::Forward), None);
        assert_eq!(state.ids_needing_metadata().len(), 0);
    }

    #[test]
    fn predicting_moves_the_current_track_and_returns_what_to_show() {
        let mut state = queue_of(&[651, 653, 655], 653);
        state.cache_track(655, metadata("Chips n Queso"));

        let predicted = state.predict(Step::Forward, 0).unwrap();
        assert_eq!(predicted.title.as_str(), "Chips n Queso");
        assert_eq!(state.current_id(), Some(655));
        assert!(state.has_prediction());
    }

    #[test]
    fn without_cached_metadata_there_is_nothing_to_show_yet() {
        let mut state = queue_of(&[651, 653, 655], 653);
        assert!(state.predict(Step::Forward, 0).is_none());
        assert!(!state.has_prediction());
        // And the current track is left alone rather than moved on a guess we
        // could not display.
        assert_eq!(state.current_id(), Some(653));
    }

    #[test]
    fn a_confirmed_prediction_is_cleared() {
        let mut state = queue_of(&[651, 653, 655], 653);
        state.cache_track(655, metadata("Chips n Queso"));
        state.predict(Step::Forward, 0).unwrap();

        assert_eq!(state.reconcile(655, 100), Reconcile::Confirmed);
        assert!(!state.has_prediction());
    }

    #[test]
    fn a_contradicted_prediction_is_held_until_it_expires() {
        let mut state = queue_of(&[651, 653, 655], 653);
        state.cache_track(655, metadata("Chips n Queso"));
        state.predict(Step::Forward, 0).unwrap();

        // The device still reports the old track: it has not caught up.
        assert_eq!(state.reconcile(653, 100), Reconcile::Holding);
        assert!(state.has_prediction());

        // Past the deadline the device wins, whatever it says.
        assert_eq!(state.reconcile(653, PREDICTION_TTL_MS), Reconcile::Expired);
        assert!(!state.has_prediction());
    }

    #[test]
    fn reconciling_without_a_prediction_is_idle() {
        let mut state = queue_of(&[651, 653], 651);
        assert_eq!(state.reconcile(653, 0), Reconcile::Idle);
    }

    #[test]
    fn only_uncached_neighbours_are_worth_fetching() {
        let mut state = queue_of(&[651, 653, 655], 653);
        let wanted = state.ids_needing_metadata();
        assert_eq!(wanted.len(), 2);
        assert!(wanted.contains(&655) && wanted.contains(&651));

        state.cache_track(655, metadata("ahead"));
        assert_eq!(state.ids_needing_metadata(), [651]);
    }

    #[test]
    fn the_cache_evicts_rather_than_growing() {
        let mut state = queue_of(&[1, 2, 3], 2);
        for id in 0..(TRACK_CACHE_LEN as u32 + 2) {
            state.cache_track(id, metadata("x"));
        }
        assert_eq!(state.cache.len(), TRACK_CACHE_LEN);
        // The oldest went first.
        assert!(state.cached(0).is_none());
        assert!(state.cached(TRACK_CACHE_LEN as u32 + 1).is_some());
    }

    #[test]
    fn an_unknown_queue_predicts_nothing() {
        let mut state = PlaylistState::new();
        assert_eq!(state.neighbour_id(Step::Forward), None);
        assert!(state.predict(Step::Forward, 0).is_none());
    }

    #[test]
    fn a_current_id_outside_the_queue_predicts_nothing() {
        let state = queue_of(&[651, 653], 999);
        assert_eq!(state.current_index(), None);
        assert_eq!(state.neighbour_id(Step::Forward), None);
    }
}
