//! Whimsical personality — animated kaomoji + contextual messages.
//!
//! Animation cycle:
//!   "agent-canopy" erases right-to-left → kaomoji flashes →
//!   message types left-to-right → holds 5-9s → erases right-to-left →
//!   brief blank → "agent-canopy" returns.

use std::time::{Duration, Instant};

// ── Timing ────────────────────────────────────────────────────────

pub const TITLE: &str = "agent-canopy";
const ERASE_MS: u64 = 35;
const TYPE_MS: u64 = 45;
const KAOMOJI_MS: u64 = 400;
const BLANK_MS: u64 = 200;
const HOLD_MIN: u64 = 4;
const HOLD_MAX: u64 = 8;
const INTERVAL_MIN: u64 = 60;
const INTERVAL_MAX: u64 = 180;
const EVENT_DECAY_SECS: u64 = 15;

// ── Kaomojis ──────────────────────────────────────────────────────

const KAO_LOADING: &[&str] = &[
    "(Ծ‸ Ծ)",
    "( ≖.≖)",
    "(◡̀_◡́)",
    "(ㆆ_ㆆ)",
    "(◉̃_᷅◉)",
    "(͠◉_◉᷅ )",
    "(◑_◑)",
    "◌◎◍",
    "(ง'̀-'́)ง",
    "(っ◕‿◕)っ",
    "(づ ◕‿◕ )づ",
    "(๑•̀ㅂ•́)و",
];
const KAO_SUCCESS: &[&str] = &[
    "(*^‿^*)",
    "(◕‿◕)",
    "(っ▀¯▀)つ",
    "ヾ(´〇`)ﾉ♪♪♪",
    "(◠﹏◠)",
    "٩(˘◡˘)۶",
    "ᕙ(`▿´)ᕗ",
    "(ᵔᵕᵔ)",
    "(๑˃ᴗ˂)ﻭ",
    "(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧",
    "(b ᵔ▽ᵔ)b",
    "٩(◕‿◕)۶",
    "(★ω★)",
];
const KAO_ERROR: &[&str] = &[
    "ಥ_ಥ",
    "◔_◔",
    "(҂◡_◡)",
    "(Ծ‸ Ծ)",
    "¯\\_(ツ)_/¯",
    "¿ⓧ_ⓧﮌ",
    "(╥﹏╥)",
    "( ˘︹˘ )",
    "(ノಠ益ಠ)ノ彡┻━┻",
    "(╯°□°）╯︵ ┻━┻",
    "(ಥ﹏ಥ)",
    "(×_×)",
];
const KAO_THINKING: &[&str] = &[
    "(ʘ_ʘ)",
    "(º_º)",
    "(￢_￢)",
    "(._.)",
    "ఠ_ఠ",
    "(⊙_◎)",
    "（´ー｀）",
    "(꜆꜄ * )꜆꜄",
    "( • ̀ω•́ )✧",
    "(￣ω￣;)",
    "(；⌣̀_⌣́)",
    "( ˘▽˘)っ旦",
    "( ͡° ͜ʖ ͡°)",
];

// ── Actions ───────────────────────────────────────────────────────

const ACT_LOADING: &[&str] = &[
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
    "Hydrating",
    "Provisioning",
    "Bootstrapping",
    "Refactoring",
    "Overclocking",
    "Transpiling",
    "Grokking",
    "Defragmenting",
];
const ACT_SUCCESS: &[&str] = &[
    "Completed",
    "Done",
    "Stabilized",
    "Resolved",
    "Deployed",
    "Confirmed",
    "Verified",
    "Shipped",
    "Unlocked",
    "Optimized",
    "Synthesized",
    "Propagated",
    "Harmonized",
    "Ascended",
];
const ACT_ERROR: &[&str] = &[
    "Something broke",
    "Signal lost",
    "Unexpected anomaly",
    "Collision detected",
    "Entropy overflow",
    "Segfault in",
    "Desynchronized",
    "Depleted",
    "Terminated",
    "Exhausted",
    "Imploded",
    "Melted",
    "Recalibrating",
    "Simplifying",
    "Accepting",
];
const ACT_THINKING: &[&str] = &[
    "Evaluating",
    "Considering",
    "Weighing",
    "Simulating",
    "Modeling",
    "Questioning",
    "Investigating",
    "Dreaming of",
    "Abstracting",
    "Inferring",
    "Meditating on",
    "Hypothesizing",
    "Optimizing",
    "Visualizing",
];

// ── Objects ───────────────────────────────────────────────────────

const OBJ_DEV: &[&str] = &[
    "the build pipeline",
    "memory leaks",
    "all dependencies",
    "the event loop",
    "parallel threads",
    "null references",
    "the type system",
    "edge cases",
    "async chaos",
    "legacy spaghetti",
    "YAML indentation",
    "the production DB",
    "unresolved PRs",
    "the borrow checker",
    "the monad",
    "the linker",
    "clean code",
    "the refactor",
];
const OBJ_SPACE: &[&str] = &[
    "cosmic background noise",
    "the event horizon",
    "orbital parameters",
    "dark matter traces",
    "parallel universes",
    "the observable scope",
    "stellar coordinates",
    "quantum foam",
    "spacetime curvature",
    "void pointers",
    "the flux capacitor",
    "the golden record",
];
const OBJ_SCIENCE: &[&str] = &[
    "entropy levels",
    "wave functions",
    "energy states",
    "the hypothesis",
    "controlled variables",
    "molecular noise",
    "the signal",
    "quantum states",
    "unknown constants",
    "the double-slit experiment",
    "Schrödinger's cat",
];
const OBJ_ABSURD: &[&str] = &[
    "the rubber duck",
    "coffee levels",
    "the cat on keyboard",
    "semicolons",
    "the D20",
    "stack overflow",
    "the intern",
    "the void",
    "common sense",
    "blinker fluid",
    "the 'it works on my machine' seal",
    "the missing bracket",
];
const OBJ_NATURE: &[&str] = &[
    "the root system",
    "fallen branches",
    "the undergrowth",
    "moss patterns",
    "the tree rings",
    "canopy layers",
    "mycelium networks",
    "wind currents",
    "leaf patterns",
    "photosynthetic efficiency",
    "the sap flow",
];
const OBJ_AI: &[&str] = &[
    "the latent space",
    "hallucination filters",
    "token budgets",
    "vector embeddings",
    "stochastic parrots",
    "RLHF feedback",
    "prompt injections",
    "overfitting tendencies",
    "the neural pathways",
];

// ── Twists ────────────────────────────────────────────────────────

const TWIST_FUNNY: &[&str] = &[
    "(probably)",
    "(don't panic)",
    "(it works on my machine)",
    "(send help)",
    "(this is fine)",
    "(might explode)",
    "(no guarantees)",
    "(fingers crossed)",
    "(legacy debt included)",
    "(at least it's not COBOL)",
    "(O(n!) efficiency)",
    "(it's a feature now)",
    "(sponsored by caffeine)",
    "(it was DNS)",
    "(allegedly)",
    "(standard procedure)",
    "(error 404: joke not found)",
    "(oops)",
];
const TWIST_POETIC: &[&str] = &[
    "across dimensions",
    "in the void",
    "beyond observable limits",
    "through the event horizon",
    "between the stars",
    "at the edge of reason",
    "in silence",
    "beyond the known",
    "under the canopy",
];
const TWIST_ADVICE: &[&str] = &[
    "— keep it simple",
    "— read the logs",
    "— don't overthink it",
    "— ship small changes",
    "— test before trusting",
    "— name things properly",
    "— fail fast",
    "— question assumptions",
    "— try turning it off and on",
    "— take a deep breath",
    "— it's just code",
];
const TWIST_CHILL: &[&str] = &[
    "smoothly",
    "with patience",
    "calmly",
    "just fine",
    "as intended",
    "all good",
    "no rush",
    "step by step",
    "in harmony",
    "perfectly",
];

// ── Direct phrases (context-driven) ──────────────────────────────

const PH_IDLE: &[&str] = &[
    "the canopy rests",
    "leaves settling",
    "photosynthesis mode",
    "listening to the forest",
    "roots are deep",
    "quiet among the branches",
    "the understory hums",
    "dappled sunlight",
    "garbage collecting dead leaves",
    "waiting for a breeze (or a task)",
    "watching the shadows move",
];
const PH_SPAWN: &[&str] = &[
    "new growth detected",
    "a seedling emerges",
    "branches extending",
    "the forest expands",
    "fresh leaves unfurling",
    "welcome to the grove",
    "git checkout -b new-branch-literally",
    "planting a new seed",
];
const PH_SUCCESS: &[&str] = &[
    "sunlight breaks through",
    "the forest hums",
    "equilibrium restored",
    "another ring in the trunk",
    "the canopy thrives",
    "fruits of labor",
    "100% test coverage (of my leaves)",
    "blooming beautifully",
    "the ecosystem is stable",
];
const PH_ERROR: &[&str] = &[
    "storm damage reported",
    "a branch gave way",
    "the wind picks up",
    "lightning struck nearby",
    "roots need attention",
    "the canopy sways hard",
    "wildfire in the server room",
    "nature finds a way",
    "a leaf fell prematurely",
    "rebalancing the soil",
];
const PH_SCROLL: &[&str] = &[
    "exploring the layers",
    "scanning tree rings",
    "tracing the bark",
    "reading the growth",
    "deeper into the forest",
    "following the grain",
    "grep-ing through the foliage",
];
const PH_BUSY: &[&str] = &[
    "the forest is alive",
    "all branches active",
    "ecosystem in full swing",
    "photosynthesis overload",
    "the canopy buzzes",
    "biodiversity peak",
    "parallel processing chlorophyll",
];

// ── Types ─────────────────────────────────────────────────────────

/// Contextual hint about what the user is doing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WhimContext {
    Idle,
    AgentSpawned,
    AgentDone,
    AgentFailed,
    TaskRunning,
    Scrolling,
    Busy,
}

/// One animation frame returned by `tick()`.
pub struct WhimFrame {
    /// How many chars of TITLE are visible (0 = hidden, `TITLE.len()` = full).
    pub title_visible: usize,
    /// The kaomoji to display (empty when title is showing).
    pub kaomoji: String,
    /// The message text (may be partially visible).
    pub text: String,
    /// How many chars of `text` are visible.
    pub text_visible: usize,
}

#[derive(Clone, Copy)]
enum Intent {
    Loading,
    Success,
    Error,
    Thinking,
}

enum Phase {
    Idle,
    ErasingTitle,
    KaomojiFlash,
    TypingMsg,
    Holding,
    ErasingMsg,
    Blank,
}

// ── PRNG ──────────────────────────────────────────────────────────

struct Rng(u64);

impl Rng {
    fn from_instant(t: Instant) -> Self {
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
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1000) < (p * 1000.0) as u64
    }
}

// ── Dedup ring ────────────────────────────────────────────────────

#[derive(Clone)]
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

fn pick_no_repeat(rng: &mut Rng, len: usize, seen: &DedupRing) -> usize {
    for _ in 0..10 {
        let i = rng.range(len);
        if !seen.contains(i) {
            return i;
        }
    }
    rng.range(len)
}

// ── Whimsg ────────────────────────────────────────────────────────

pub struct Whimsg {
    rng: Rng,
    phase: Phase,
    phase_start: Instant,
    next_trigger: Instant,
    active_kaomoji: String,
    active_text: String,
    active_hold_ms: u64,
    event_context: Option<WhimContext>,
    event_at: Instant,
    ambient: WhimContext,
    seen_kaomoji: DedupRing,
    seen_action: DedupRing,
    seen_object: DedupRing,
    seen_twist: DedupRing,
    seen_phrase: DedupRing,
}

impl Whimsg {
    pub fn new() -> Self {
        let mut rng = Rng::from_instant(Instant::now());
        let first = Duration::from_secs(rng.between(8, 20));
        Self {
            phase: Phase::Idle,
            phase_start: Instant::now(),
            next_trigger: Instant::now() + first,
            active_kaomoji: String::new(),
            active_text: String::new(),
            active_hold_ms: 0,
            event_context: None,
            event_at: Instant::now() - Duration::from_secs(999),
            ambient: WhimContext::Idle,
            seen_kaomoji: DedupRing::new(8),
            seen_action: DedupRing::new(8),
            seen_object: DedupRing::new(8),
            seen_twist: DedupRing::new(8),
            seen_phrase: DedupRing::new(8),
            rng,
        }
    }

    /// Set the ambient context (reflects ongoing state: idle, busy, etc.).
    pub fn set_ambient(&mut self, ctx: WhimContext) {
        self.ambient = ctx;
    }

    /// Push a one-shot event (spawn, exit, error). Triggers a sooner message.
    pub fn notify_event(&mut self, event: WhimContext) {
        self.event_context = Some(event);
        self.event_at = Instant::now();
        if matches!(self.phase, Phase::Idle) {
            let soon = self.rng.between(15, 30);
            let proposed = Instant::now() + Duration::from_secs(soon);
            if proposed < self.next_trigger {
                self.next_trigger = proposed;
            }
        }
    }

    /// Produce the current animation frame. Call every render tick.
    pub fn tick(&mut self) -> WhimFrame {
        loop {
            let elapsed = self.phase_start.elapsed().as_millis() as u64;
            match self.phase {
                Phase::Idle => {
                    if Instant::now() >= self.next_trigger {
                        self.generate();
                        self.advance(Phase::ErasingTitle);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: TITLE.len(),
                        kaomoji: String::new(),
                        text: String::new(),
                        text_visible: 0,
                    };
                }
                Phase::ErasingTitle => {
                    let erased = (elapsed / ERASE_MS) as usize;
                    if erased >= TITLE.len() {
                        self.advance(Phase::KaomojiFlash);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: TITLE.len() - erased,
                        kaomoji: String::new(),
                        text: String::new(),
                        text_visible: 0,
                    };
                }
                Phase::KaomojiFlash => {
                    if elapsed >= KAOMOJI_MS {
                        self.advance(Phase::TypingMsg);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: 0,
                        kaomoji: self.active_kaomoji.clone(),
                        text: String::new(),
                        text_visible: 0,
                    };
                }
                Phase::TypingMsg => {
                    let total = self.active_text.chars().count();
                    let typed = (elapsed / TYPE_MS) as usize;
                    if typed >= total {
                        self.advance(Phase::Holding);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: 0,
                        kaomoji: self.active_kaomoji.clone(),
                        text: self.active_text.clone(),
                        text_visible: typed,
                    };
                }
                Phase::Holding => {
                    if elapsed >= self.active_hold_ms {
                        self.advance(Phase::ErasingMsg);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: 0,
                        kaomoji: self.active_kaomoji.clone(),
                        text: self.active_text.clone(),
                        text_visible: self.active_text.chars().count(),
                    };
                }
                Phase::ErasingMsg => {
                    let total = self.active_text.chars().count();
                    let erased = (elapsed / ERASE_MS) as usize;
                    if erased >= total {
                        self.advance(Phase::Blank);
                        continue;
                    }
                    return WhimFrame {
                        title_visible: 0,
                        kaomoji: self.active_kaomoji.clone(),
                        text: self.active_text.clone(),
                        text_visible: total - erased,
                    };
                }
                Phase::Blank => {
                    if elapsed >= BLANK_MS {
                        let delay = self.rng.between(INTERVAL_MIN, INTERVAL_MAX);
                        self.next_trigger = Instant::now() + Duration::from_secs(delay);
                        self.advance(Phase::Idle);
                        return WhimFrame {
                            title_visible: TITLE.len(),
                            kaomoji: String::new(),
                            text: String::new(),
                            text_visible: 0,
                        };
                    }
                    return WhimFrame {
                        title_visible: 0,
                        kaomoji: String::new(),
                        text: String::new(),
                        text_visible: 0,
                    };
                }
            }
        }
    }

    fn advance(&mut self, next: Phase) {
        self.phase = next;
        self.phase_start = Instant::now();
    }

    fn active_context(&self) -> WhimContext {
        if let Some(ctx) = self.event_context {
            if self.event_at.elapsed() < Duration::from_secs(EVENT_DECAY_SECS) {
                return ctx;
            }
        }
        self.ambient
    }

    fn generate(&mut self) {
        let ctx = self.active_context();
        let intent = self.pick_intent(ctx);

        // Always pick kaomoji (100%)
        let kaomojis = match intent {
            Intent::Loading => KAO_LOADING,
            Intent::Success => KAO_SUCCESS,
            Intent::Error => KAO_ERROR,
            Intent::Thinking => KAO_THINKING,
        };
        let ki = pick_no_repeat(&mut self.rng, kaomojis.len(), &self.seen_kaomoji);
        self.seen_kaomoji.push(ki);
        self.active_kaomoji = kaomojis[ki].to_string();

        // 30% chance of a direct context-driven phrase
        if self.rng.chance(0.30) {
            let phrases = match ctx {
                WhimContext::Idle => PH_IDLE,
                WhimContext::AgentSpawned => PH_SPAWN,
                WhimContext::AgentDone => PH_SUCCESS,
                WhimContext::AgentFailed => PH_ERROR,
                WhimContext::TaskRunning => PH_BUSY,
                WhimContext::Scrolling => PH_SCROLL,
                WhimContext::Busy => PH_BUSY,
            };
            let pi = pick_no_repeat(&mut self.rng, phrases.len(), &self.seen_phrase);
            self.seen_phrase.push(pi);
            self.active_text = phrases[pi].to_string();
        } else {
            // Template: action + object + twist
            let actions = match intent {
                Intent::Loading => ACT_LOADING,
                Intent::Success => ACT_SUCCESS,
                Intent::Error => ACT_ERROR,
                Intent::Thinking => ACT_THINKING,
            };
            let domain = self.rng.range(6);
            let objects = match domain {
                0 => OBJ_DEV,
                1 => OBJ_SPACE,
                2 => OBJ_SCIENCE,
                3 => OBJ_NATURE,
                4 => OBJ_AI,
                _ => OBJ_ABSURD,
            };
            let style = self.rng.range(5);
            let twists: &[&str] = match style {
                0 => TWIST_FUNNY,
                1 => TWIST_POETIC,
                2 => TWIST_ADVICE,
                3 => TWIST_CHILL,
                _ => &["..."],
            };

            let ai = pick_no_repeat(&mut self.rng, actions.len(), &self.seen_action);
            self.seen_action.push(ai);
            let oi = pick_no_repeat(&mut self.rng, objects.len(), &self.seen_object);
            self.seen_object.push(oi);
            let ti = pick_no_repeat(&mut self.rng, twists.len(), &self.seen_twist);
            self.seen_twist.push(ti);
            self.active_text = format!("{} {} {}", actions[ai], objects[oi], twists[ti]);
        }

        self.active_hold_ms = self.rng.between(HOLD_MIN, HOLD_MAX) * 1000;
    }

    fn pick_intent(&mut self, ctx: WhimContext) -> Intent {
        match ctx {
            WhimContext::Idle => match self.rng.range(10) {
                0..=3 => Intent::Thinking,
                4..=6 => Intent::Loading,
                _ => Intent::Success,
            },
            WhimContext::AgentSpawned => {
                if self.rng.chance(0.8) {
                    Intent::Success
                } else {
                    Intent::Loading
                }
            }
            WhimContext::AgentDone => {
                if self.rng.chance(0.9) {
                    Intent::Success
                } else {
                    Intent::Thinking
                }
            }
            WhimContext::AgentFailed => {
                // Balance errors: 40% error, 40% thinking (pondering), 20% hopeful/success
                match self.rng.range(10) {
                    0..=3 => Intent::Error,
                    4..=7 => Intent::Thinking,
                    _ => Intent::Success,
                }
            }
            WhimContext::TaskRunning => {
                if self.rng.chance(0.7) {
                    Intent::Loading
                } else {
                    Intent::Thinking
                }
            }
            WhimContext::Scrolling => {
                if self.rng.chance(0.7) {
                    Intent::Thinking
                } else {
                    Intent::Loading
                }
            }
            WhimContext::Busy => {
                if self.rng.chance(0.7) {
                    Intent::Loading
                } else {
                    Intent::Thinking
                }
            }
        }
    }
}
