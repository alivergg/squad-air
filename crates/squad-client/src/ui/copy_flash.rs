use std::time::{Duration, Instant};

pub const COPY_FLASH_DURATION: Duration = Duration::from_secs(2);
pub const COPY_FLASH_IN_DURATION: Duration = Duration::from_millis(240);
pub const COPY_FLASH_OUT_DURATION: Duration = Duration::from_millis(240);
pub const COPY_FLASH_FADE_START: Duration = Duration::from_millis(1760);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CopyFlashProgress {
    pub intensity: f32,
    pub check_draw: f32,
    pub pop_pulse: f32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyFlashState {
    pub invite_at: Option<Instant>,
    pub friend_id_at: Option<Instant>,
    pub short_code_at: Option<Instant>,
    pub chat_msg: Option<(u64, Instant)>,
    pub image_blob: Option<([u8; 16], Instant)>,
}

impl CopyFlashState {
    pub fn mark_invite_at(&mut self, now: Instant) {
        self.invite_at = Some(now);
    }

    pub fn mark_friend_id_at(&mut self, now: Instant) {
        self.friend_id_at = Some(now);
    }

    pub fn mark_short_code_at(&mut self, now: Instant) {
        self.short_code_at = Some(now);
    }

    pub fn mark_chat_msg_at(&mut self, msg_id: u64, now: Instant) {
        self.chat_msg = Some((msg_id, now));
    }

    pub fn mark_image_at(&mut self, blob_id: [u8; 16], now: Instant) {
        self.image_blob = Some((blob_id, now));
    }

    pub fn clear_invite(&mut self) {
        self.invite_at = None;
    }

    pub fn clear_friend_id(&mut self) {
        self.friend_id_at = None;
    }

    pub fn clear_short_code(&mut self) {
        self.short_code_at = None;
    }

    pub fn clear_room_flashes(&mut self) {
        self.invite_at = None;
        self.short_code_at = None;
        self.chat_msg = None;
        self.image_blob = None;
    }

    pub fn clear_expired_at(&mut self, now: Instant) {
        let expired = |ts: Instant| now.saturating_duration_since(ts) >= COPY_FLASH_DURATION;
        if self.invite_at.is_some_and(expired) {
            self.invite_at = None;
        }
        if self.friend_id_at.is_some_and(expired) {
            self.friend_id_at = None;
        }
        if self.short_code_at.is_some_and(expired) {
            self.short_code_at = None;
        }
        if self.chat_msg.is_some_and(|(_, ts)| expired(ts)) {
            self.chat_msg = None;
        }
        if self.image_blob.is_some_and(|(_, ts)| expired(ts)) {
            self.image_blob = None;
        }
    }

    #[cfg(test)]
    pub fn has_any_active(&self) -> bool {
        self.invite_at.is_some()
            || self.friend_id_at.is_some()
            || self.short_code_at.is_some()
            || self.chat_msg.is_some()
            || self.image_blob.is_some()
    }

    pub fn needs_animation_tick_at(&self, now: Instant) -> bool {
        let animating = |ts: Instant| {
            let elapsed = now.saturating_duration_since(ts);
            elapsed < COPY_FLASH_IN_DURATION
                || (elapsed >= COPY_FLASH_FADE_START && elapsed < COPY_FLASH_DURATION)
        };
        self.invite_at.is_some_and(animating)
            || self.friend_id_at.is_some_and(animating)
            || self.short_code_at.is_some_and(animating)
            || self.chat_msg.is_some_and(|(_, ts)| animating(ts))
            || self.image_blob.is_some_and(|(_, ts)| animating(ts))
    }

    pub fn progress_for(ts: Instant, now: Instant) -> Option<CopyFlashProgress> {
        let elapsed = now.saturating_duration_since(ts);
        if elapsed >= COPY_FLASH_DURATION {
            return None;
        }
        if elapsed < COPY_FLASH_IN_DURATION {
            let u = (elapsed.as_secs_f32() / COPY_FLASH_IN_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - u).powi(3);
            let pop_pulse = (u * std::f32::consts::PI).sin().max(0.0);
            Some(CopyFlashProgress {
                intensity: ease,
                check_draw: ease,
                pop_pulse,
            })
        } else if elapsed < COPY_FLASH_FADE_START {
            Some(CopyFlashProgress {
                intensity: 1.0,
                check_draw: 1.0,
                pop_pulse: 0.0,
            })
        } else {
            let v = ((elapsed - COPY_FLASH_FADE_START).as_secs_f32()
                / COPY_FLASH_OUT_DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
            let fade = 1.0 - v.powi(2);
            Some(CopyFlashProgress {
                intensity: fade,
                check_draw: 1.0,
                pop_pulse: 0.0,
            })
        }
    }

    pub fn invite_progress_at(&self, now: Instant) -> Option<CopyFlashProgress> {
        self.invite_at.and_then(|ts| Self::progress_for(ts, now))
    }

    pub fn friend_id_progress_at(&self, now: Instant) -> Option<CopyFlashProgress> {
        self.friend_id_at.and_then(|ts| Self::progress_for(ts, now))
    }

    pub fn short_code_progress_at(&self, now: Instant) -> Option<CopyFlashProgress> {
        self.short_code_at
            .and_then(|ts| Self::progress_for(ts, now))
    }

    pub fn chat_msg_progress_at(&self, msg_id: u64, now: Instant) -> Option<CopyFlashProgress> {
        self.chat_msg
            .filter(|(id, _)| *id == msg_id)
            .and_then(|(_, ts)| Self::progress_for(ts, now))
    }

    pub fn image_progress_at(&self, blob_id: [u8; 16], now: Instant) -> Option<CopyFlashProgress> {
        self.image_blob
            .filter(|(id, _)| *id == blob_id)
            .and_then(|(_, ts)| Self::progress_for(ts, now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_query_all_five_copy_targets() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        assert!(!state.has_any_active());

        let blob = [7u8; 16];
        state.mark_invite_at(t0);
        state.mark_friend_id_at(t0);
        state.mark_short_code_at(t0);
        state.mark_chat_msg_at(42, t0);
        state.mark_image_at(blob, t0);

        assert!(state.has_any_active());
        assert!(state.invite_progress_at(t0).is_some());
        assert!(state.friend_id_progress_at(t0).is_some());
        assert!(state.short_code_progress_at(t0).is_some());
        assert!(state.chat_msg_progress_at(42, t0).is_some());
        assert!(state.chat_msg_progress_at(99, t0).is_none());
        assert!(state.image_progress_at(blob, t0).is_some());
        assert!(state.image_progress_at([8u8; 16], t0).is_none());
    }

    #[test]
    fn does_not_expire_before_2s_and_expires_at_2s() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        let blob = [3u8; 16];
        state.mark_invite_at(t0);
        state.mark_friend_id_at(t0);
        state.mark_short_code_at(t0);
        state.mark_chat_msg_at(10, t0);
        state.mark_image_at(blob, t0);

        let before = t0 + Duration::from_millis(1900);
        state.clear_expired_at(before);
        assert!(state.has_any_active());
        assert!(state.invite_progress_at(before).is_some());
        assert!(state.friend_id_progress_at(before).is_some());
        assert!(state.short_code_progress_at(before).is_some());
        assert!(state.chat_msg_progress_at(10, before).is_some());
        assert!(state.image_progress_at(blob, before).is_some());

        let at_2s = t0 + COPY_FLASH_DURATION;
        assert!(state.invite_progress_at(at_2s).is_none());
        assert!(state.friend_id_progress_at(at_2s).is_none());
        assert!(state.short_code_progress_at(at_2s).is_none());
        assert!(state.chat_msg_progress_at(10, at_2s).is_none());
        assert!(state.image_progress_at(blob, at_2s).is_none());

        state.clear_expired_at(at_2s);
        assert!(!state.has_any_active());
    }

    #[test]
    fn reclick_within_2s_extends_deadline_against_stale_timer() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        let blob = [11u8; 16];
        state.mark_invite_at(t0);
        state.mark_chat_msg_at(5, t0);
        state.mark_image_at(blob, t0);

        let reclick = t0 + Duration::from_millis(1100);
        state.mark_invite_at(reclick);
        state.mark_chat_msg_at(5, reclick);
        state.mark_image_at(blob, reclick);

        let first_timer = t0 + COPY_FLASH_DURATION;
        state.clear_expired_at(first_timer);
        assert!(state.invite_progress_at(first_timer).is_some());
        assert!(state.chat_msg_progress_at(5, first_timer).is_some());
        assert!(state.image_progress_at(blob, first_timer).is_some());

        let just_before_second = reclick + Duration::from_millis(1999);
        state.clear_expired_at(just_before_second);
        assert!(state.invite_progress_at(just_before_second).is_some());
        assert!(state.chat_msg_progress_at(5, just_before_second).is_some());
        assert!(state.image_progress_at(blob, just_before_second).is_some());

        let second_timer = reclick + COPY_FLASH_DURATION;
        state.clear_expired_at(second_timer);
        assert!(!state.has_any_active());
    }

    #[test]
    fn independent_targets_expire_independently() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        state.mark_invite_at(t0);
        state.mark_friend_id_at(t0 + Duration::from_millis(600));

        state.clear_expired_at(t0 + COPY_FLASH_DURATION);
        assert!(state.invite_at.is_none());
        assert!(state.friend_id_at.is_some());

        state.clear_expired_at(t0 + Duration::from_millis(2600));
        assert!(state.friend_id_at.is_none());
        assert!(!state.has_any_active());
    }

    #[test]
    fn animation_tick_windows_and_idle_zero_cost() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        assert!(!state.needs_animation_tick_at(t0));

        state.mark_invite_at(t0);

        let p_start = state.invite_progress_at(t0).unwrap();
        let p_mid = state
            .invite_progress_at(t0 + Duration::from_millis(120))
            .unwrap();
        let p_end_in = state
            .invite_progress_at(t0 + Duration::from_millis(239))
            .unwrap();
        assert!(state.needs_animation_tick_at(t0));
        assert!(state.needs_animation_tick_at(t0 + Duration::from_millis(120)));
        assert!(state.needs_animation_tick_at(t0 + Duration::from_millis(239)));
        assert!(p_start.intensity < p_mid.intensity && p_mid.intensity < p_end_in.intensity);
        assert!(p_mid.pop_pulse > 0.0);

        for ms in [240u64, 1000, 1759] {
            let t = t0 + Duration::from_millis(ms);
            assert!(!state.needs_animation_tick_at(t));
            let p = state.invite_progress_at(t).unwrap();
            assert_eq!(p.intensity, 1.0);
            assert_eq!(p.check_draw, 1.0);
            assert_eq!(p.pop_pulse, 0.0);
        }

        let t_fade_start = t0 + Duration::from_millis(1760);
        let t_fade_mid = t0 + Duration::from_millis(1900);
        assert!(state.needs_animation_tick_at(t_fade_start));
        assert!(state.needs_animation_tick_at(t_fade_mid));
        let p_fade_start = state.invite_progress_at(t_fade_start).unwrap();
        let p_fade_mid = state.invite_progress_at(t_fade_mid).unwrap();
        assert!(p_fade_mid.intensity < p_fade_start.intensity);
        assert!(p_fade_mid.intensity > 0.0);

        let t_done = t0 + Duration::from_millis(2000);
        assert!(!state.needs_animation_tick_at(t_done));
        assert!(state.invite_progress_at(t_done).is_none());
    }

    #[test]
    fn room_and_modal_reset_behavior() {
        let t0 = Instant::now();
        let mut state = CopyFlashState::default();
        state.mark_invite_at(t0);
        state.mark_friend_id_at(t0);
        state.mark_short_code_at(t0);
        state.mark_chat_msg_at(1, t0);
        state.mark_image_at([9u8; 16], t0);

        state.clear_short_code();
        assert!(state.short_code_at.is_none());
        assert!(state.invite_at.is_some());

        state.clear_room_flashes();
        assert!(state.invite_at.is_none());
        assert!(state.short_code_at.is_none());
        assert!(state.chat_msg.is_none());
        assert!(state.image_blob.is_none());
        assert!(state.friend_id_at.is_some());

        state.clear_friend_id();
        assert!(state.friend_id_at.is_none());
        assert!(!state.has_any_active());
    }
}
