//! Whimsical message generator — gives canopy a personality.
//!
//! Periodically replaces the "agent-canopy" header with a kaomoji + phrase
//! that fades back after a few seconds.

use std::time::{Duration, Instant};

/// How long a whimsg stays visible before fading back to the title.
const DISPLAY_DURATION: Duration = Duration::from_secs(5);

/// Minimum interval between whimsgs (avoids spam).
const MIN_INTERVAL: Duration = Duration::from_secs(15);

/// Maximum interval between whimsgs.
const MAX_INTERVAL: Duration = Duration::from_secs(45);

/// How many recent items to remember per slot for dedup.
const DEDUP_BUF: usize = 8;

// ── Datasets ──────────────────────────────────────────────────────

const KAOMOJIS_LOADING: &[&str] = &[
    "(Ծ‸ Ծ)",
    "( ≖.≖)",
    "(◡̀_◡́)",
    "(ㆆ_ㆆ)",
    "(◉̃_᷅◉)",
    "(͠◉_◉᷅ )",
    "(◑_◑)",
];

const KAOMOJIS_SUCCESS: &[&str] = &[
    "(♥‿♥)",
    "(◕‿◕)",
    "(っ▀¯▀)つ",
    "ヾ(´〇`)ﾉ♪♪♪",
    "(◠﹏◠)",
    "٩(˘◡˘)۶",
    "ᕙ(`▿´)ᕗ",
];

const KAOMOJIS_ERROR: &[&str] = &[
    "ಥ_ಥ",
    "◔_◔",
    "(҂◡_◡)",
    "♨_♨",
    "(Ծ‸ Ծ)",
    "¯\\_(ツ)_/¯",
    "¿ⓧ_ⓧﮌ",
    "(╥﹏╥)",
    "( ˘︹˘ )",
];

const KAOMOJIS_THINKING: &[&str] = &[
    "(ʘ_ʘ)",
    "(º_º)",
    "(￢_￢)",
    "(._.)",
    "ఠ_ఠ",
    "(⊙_◎)",
];

const ACTIONS_LOADING: &[&str] = &[
    "Calibrating",
    "Aligning",
    "Resolving",
    "Processing",
    "Exploring",
    "Parsing",
    "Synchronizing",
    "Mapping",
    "Scanning",
    "Warming up",
];

const ACTIONS_SUCCESS: &[&str] = &[
    "Completed",
    "Done",
    "Stabilized",
    "Resolved",
    "Deployed",
    "Confirmed",
    "Verified",
    "Shipped",
    "Unlocked",
];

const ACTIONS_ERROR: &[&str] = &[
    "Something broke",
    "Signal lost",
    "Unexpected anomaly",
    "Collision detected",
    "Entropy overflow",
    "Segfault in",
];

const ACTIONS_THINKING: &[&str] = &[
    "Evaluating",
    "Considering",
    "Weighing",
    "Simulating",
    "Modeling",
    "Questioning",
    "Investigating",
];

const OBJECTS_DEV: &[&str] = &[
    "the build pipeline",
    "memory leaks",
    "all dependencies",
    "the event loop",
    "parallel threads",
    "null references",
    "the type system",
    "edge cases",
    "async chaos",
];

const OBJECTS_SPACE: &[&str] = &[
    "cosmic background noise",
    "the event horizon",
    "orbital parameters",
    "dark matter traces",
    "parallel universes",
    "the observable scope",
    "stellar coordinates",
    "quantum foam",
    "spacetime curvature",
];

const OBJECTS_SCIENCE: &[&str] = &[
    "entropy levels",
    "wave functions",
    "energy states",
    "the hypothesis",
    "controlled variables",
    "molecular noise",
    "the signal",
    "quantum states",
    "unknown constants",
];

const OBJECTS_ABSURD: &[&str] = &[
    "the rubber duck",
    "coffee levels",
    "the cat on keyboard",
    "semicolons",
    "the D20",
    "stack overflow",
    "the intern",
    "the void",
    "common sense",
];

const TWISTS_FUNNY: &[&str] = &[
    "(probably)",
    "(don't panic)",
    "(it works on my machine)",
    "(send help)",
    "(this is fine)",
    "(might explode)",
    "(no guarantees)",
    "(fingers crossed)",
];

const TWISTS_POETIC: &[&str] = &[
    "across dimensions",
    "in the void",
    "beyond observable limits",
    "through the event horizon",
    "between the stars",
    "at the edge of reason",
    "in silence",
    "beyond the known",
];

const TWISTS_ADVICE: &[&str] = &[
    "— keep it simple",
    "— read the logs",
    "— don't overthink it",
    "— ship small changes",
    "— test before trusting",
    "— name things properly",
    "— fail fast",
    "— question assumptions",
];

// ── PRNG ──────────────────────────────────────────────────────────

/// Minimal xorshift64 — no external dependency, deterministic but chaotic.
struct Rng(u64);

impl Rng {
    fn from_instant(t: Instant) -> Self {
        // Mix the elapsed nanos since process start with a constant
        let seed = t.elapsed().as_nanos() as u64 ^ 0xDEAD_BEEF_CAFE_BABE;
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        (self.next() % max as u64) as usize
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    fn chance(&mut self, probability: f64) -> bool {
        (self.next() % 1000) < (probability * 1000.0) as u64
    }
}

// ── Dedup ring buffer ─────────────────────────────────────────────

struct DedupRing {
    buf: Vec<usize>,
    cap: usize,
}

impl DedupRing {
    fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            cap,
        }
    }

    fn contains(&self, idx: usize) -> bool {
        self.buf.contains(&idx)
    }

    fn push(&mut self, idx: usize) {
        if self.buf.len() >= self.cap {
            self.buf.remove(0);
        }
        self.buf.push(idx);
    }
}

// ── Public state ──────────────────────────────────────────────────

pub struct Whimsg {
    rng: Rng,
    /// Currently displayed message (None = show normal title).
    current: Option<String>,
    /// When the current message was set.
    shown_at: Instant,
    /// When the next message should appear.
    next_trigger: Instant,
    /// Dedup rings per slot.
    seen_kaomoji: DedupRing,
    seen_action: DedupRing,
    seen_object: DedupRing,
    seen_twist: DedupRing,
}

impl Whimsg {
    pub fn new() -> Self {
        let mut rng = Rng::from_instant(Instant::now());
        let first_delay = Duration::from_secs(rng.between(8, 20));
        Self {
            current: None,
            shown_at: Instant::now(),
            next_trigger: Instant::now() + first_delay,
            seen_kaomoji: DedupRing::new(DEDUP_BUF),
            seen_action: DedupRing::new(DEDUP_BUF),
            seen_object: DedupRing::new(DEDUP_BUF),
            seen_twist: DedupRing::new(DEDUP_BUF),
            rng,
        }
    }

    /// Called every tick. Returns the text to display in the header:
    /// `Some(whimsg)` when a message is active, `None` for default title.
    pub fn tick(&mut self) -> Option<&str> {
        let now = Instant::now();

        // If a message is showing, check if it should expire
        if self.current.is_some() {
            if now.duration_since(self.shown_at) >= DISPLAY_DURATION {
                self.current = None;
                // Schedule next appearance
                let delay_secs = self.rng.between(
                    MIN_INTERVAL.as_secs(),
                    MAX_INTERVAL.as_secs(),
                );
                self.next_trigger = now + Duration::from_secs(delay_secs);
            }
            return self.current.as_deref();
        }

        // Check if it's time to show a new message
        if now >= self.next_trigger {
            let msg = self.generate();
            self.current = Some(msg);
            self.shown_at = now;
        }

        self.current.as_deref()
    }

    fn generate(&mut self) -> String {
        // Pick random intent
        let intent = self.rng.range(4);
        // Pick random domain
        let domain = self.rng.range(4);
        // Pick random style
        let style = self.rng.range(4);

        let kaomojis = match intent {
            0 => KAOMOJIS_LOADING,
            1 => KAOMOJIS_SUCCESS,
            2 => KAOMOJIS_ERROR,
            _ => KAOMOJIS_THINKING,
        };

        let actions = match intent {
            0 => ACTIONS_LOADING,
            1 => ACTIONS_SUCCESS,
            2 => ACTIONS_ERROR,
            _ => ACTIONS_THINKING,
        };

        let objects = match domain {
            0 => OBJECTS_DEV,
            1 => OBJECTS_SPACE,
            2 => OBJECTS_SCIENCE,
            _ => OBJECTS_ABSURD,
        };

        let twists: &[&str] = match style {
            0 => TWISTS_FUNNY,
            1 => TWISTS_POETIC,
            2 => TWISTS_ADVICE,
            _ => &["..."],
        };

        // Pick with dedup
        let ki = self.pick_dedup(kaomojis.len(), &self.seen_kaomoji.clone());
        self.seen_kaomoji.push(ki);
        let ai = self.pick_dedup(actions.len(), &self.seen_action.clone());
        self.seen_action.push(ai);
        let oi = self.pick_dedup(objects.len(), &self.seen_object.clone());
        self.seen_object.push(oi);
        let ti = self.pick_dedup(twists.len(), &self.seen_twist.clone());
        self.seen_twist.push(ti);

        // Decide kaomoji visibility (65% chance)
        let show_kaomoji = self.rng.chance(0.65);

        if show_kaomoji {
            format!(
                "{}  {} {} {}",
                kaomojis[ki], actions[ai], objects[oi], twists[ti]
            )
        } else {
            format!("{} {} {}", actions[ai], objects[oi], twists[ti])
        }
    }

    fn pick_dedup(&mut self, pool_len: usize, seen: &DedupRing) -> usize {
        // Try up to 10 times to find an unseen index
        for _ in 0..10 {
            let idx = self.rng.range(pool_len);
            if !seen.contains(idx) {
                return idx;
            }
        }
        // Fallback: just pick randomly
        self.rng.range(pool_len)
    }
}

impl Clone for DedupRing {
    fn clone(&self) -> Self {
        Self {
            buf: self.buf.clone(),
            cap: self.cap,
        }
    }
}
