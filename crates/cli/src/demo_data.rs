use chrono::{Days, Local};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

const DEMO_TAG: &str = "minutes-demo-seed";
const SNOWCRASH_TAG: &str = "snow-crash";

// ── ANSI escape helpers ────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const WHITE: &str = "\x1b[97m";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
const ERASE_LINE: &str = "\x1b[K";

fn tty() -> bool {
    io::stderr().is_terminal()
}

fn pause(ms: u64) {
    if tty() {
        thread::sleep(Duration::from_millis(ms));
    }
}

fn typewriter(w: &mut impl Write, text: &str, char_ms: u64) {
    if tty() && char_ms > 0 {
        for ch in text.chars() {
            write!(w, "{}", ch).ok();
            w.flush().ok();
            thread::sleep(Duration::from_millis(char_ms));
        }
    } else {
        write!(w, "{}", text).ok();
    }
}

fn rule(w: &mut impl Write, ch: char, width: usize) {
    let line: String = std::iter::repeat_n(ch, width).collect();
    writeln!(w, "{DIM}{}{RESET}", line).ok();
}

fn term_width() -> usize {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        unsafe {
            let mut ws = MaybeUninit::<libc::winsize>::uninit();
            if libc::ioctl(2, libc::TIOCGWINSZ, ws.as_mut_ptr()) == 0 {
                let ws = ws.assume_init();
                if ws.ws_col > 0 {
                    return (ws.ws_col as usize).clamp(40, 120);
                }
            }
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(40, 120)
}

fn scanning_animation(w: &mut impl Write) {
    if tty() {
        let scanned = [
            "Metaverse Infrastructure Standup",
            "CosaNostra Delivery Logistics",
            "Avatar Expression System",
            "Rat Things Perimeter Security",
            "Intel Debrief",
        ];
        for (i, name) in scanned.iter().enumerate() {
            write!(w, "\r{ERASE_LINE}  {DIM}Reading {}{RESET}", name).ok();
            w.flush().ok();
            thread::sleep(Duration::from_millis(if i == 0 { 200 } else { 120 }));
        }
        writeln!(w, "\r{ERASE_LINE}").ok();
    } else {
        writeln!(w, "  Reading 5 meetings...").ok();
        writeln!(w).ok();
    }
}

/// The main animated presentation after seeding demo meetings.
pub fn present_demo(meeting_count: usize, people_count: usize, _output_dir: &Path) {
    let mut w = io::stderr();
    let is_tty = tty();
    let width = if is_tty { term_width() } else { 80 };

    if is_tty {
        write!(w, "{HIDE_CURSOR}").ok();
    }

    // ── Header ──────────────────────────────────────────────

    writeln!(w).ok();
    let version = env!("CARGO_PKG_VERSION");
    writeln!(
        w,
        "  {BOLD}{WHITE}MINUTES{RESET} {DIM}v{version}{RESET}  {DIM}demo universe: {RESET}{CYAN}{BOLD}Snow Crash{RESET}"
    )
    .ok();
    writeln!(w).ok();

    // ── Card data ────────────────────────────────────────────

    struct MeetingCard {
        title: &'static str,
        days_ago: u32,
        people: &'static str,
        teaser: &'static str,
    }

    let cards = [
        MeetingCard {
            title: "Metaverse Infrastructure Standup",
            days_ago: 14,
            people: "Hiro Protagonist, Da5id Meier",
            teaser: "\"I can fix it by Friday.\"",
        },
        MeetingCard {
            title: "CosaNostra Delivery Logistics",
            days_ago: 11,
            people: "Y.T., Uncle Enzo",
            teaser: "\"The 30-minute guarantee is sacred.\"",
        },
        MeetingCard {
            title: "Avatar Expression System",
            days_ago: 7,
            people: "Hiro Protagonist, Juanita Marquez",
            teaser: "\"Last I heard he was at the Raft.\"",
        },
        MeetingCard {
            title: "Rat Things Perimeter Security",
            days_ago: 4,
            people: "Mr. Lee, Ng, Hiro Protagonist",
            teaser:
                "\"Anything running through that corridor is going to hit the new checkpoints.\"",
        },
        MeetingCard {
            title: "Intel Debrief",
            days_ago: 1,
            people: "Hiro Protagonist, Y.T.",
            teaser: "\"He told me he was heads-down coding. He's been at the Raft.\"",
        },
    ];

    // ── Timeline cards ──────────────────────────────────────

    for (i, card) in cards.iter().enumerate().take(meeting_count) {
        pause(if i == 0 { 60 } else { 100 });

        let day_label = format!("{}d", card.days_ago);
        writeln!(
            w,
            "  {DIM}{:>3}{RESET} {DIM}\u{2500}\u{2500}{RESET} {WHITE}{}{RESET}",
            day_label, card.title
        )
        .ok();
        writeln!(w, "       {DIM}{}{RESET}", card.people).ok();
        writeln!(w, "       {ITALIC}{}{RESET}", card.teaser).ok();

        if i < meeting_count - 1 {
            writeln!(w, "    {DIM}\u{2502}{RESET}").ok();
        } else {
            writeln!(w).ok();
        }
        w.flush().ok();
    }

    // ── Summary stats ───────────────────────────────────────

    pause(200);
    if is_tty {
        rule(&mut w, '\u{2500}', width.min(72));
    }
    writeln!(
        w,
        "  {BOLD}{WHITE}{} people{RESET}  {DIM}\u{00b7}{RESET}  {BOLD}{WHITE}{} meetings{RESET}  {DIM}\u{00b7}{RESET}  {BOLD}{WHITE}2 threads{RESET}",
        people_count, meeting_count
    )
    .ok();
    writeln!(w).ok();

    // ── Interactive thread picker ────────────────────────────

    let interactive = is_tty && io::stdin().is_terminal();

    if interactive {
        pause(200);
        writeln!(
            w,
            "  {DIM}Two threads run through these meetings. Neither was discussed directly.{RESET}"
        )
        .ok();
        writeln!(w).ok();
        writeln!(
            w,
            "  {WHITE}{BOLD}[1]{RESET} {WHITE}The Da5id Problem{RESET} {DIM}-- a commitment traced across 3 meetings{RESET}"
        )
        .ok();
        writeln!(
            w,
            "  {WHITE}{BOLD}[2]{RESET} {WHITE}The Territory Collision{RESET} {DIM}-- a security change about to wreck a delivery route{RESET}"
        )
        .ok();
        writeln!(w).ok();
        write!(
            w,
            "  {WHITE}Pick a thread ({BOLD}1{RESET}{WHITE}/{BOLD}2{RESET}{WHITE}), or {BOLD}q{RESET}{WHITE} to explore on your own {RESET}"
        )
        .ok();
        write!(w, "{SHOW_CURSOR}").ok();
        w.flush().ok();

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input).ok();
        let choice = input.trim().to_lowercase();

        write!(w, "{HIDE_CURSOR}").ok();
        w.flush().ok();

        match choice.as_str() {
            "q" | "quit" | "n" | "no" => {
                writeln!(w).ok();
                present_explore_commands(&mut w);
                writeln!(
                    w,
                    "  {DIM}Missed the threads? Run {RESET}{CYAN}minutes demo --clean --full{RESET}{DIM} to replay.{RESET}"
                )
                .ok();
                writeln!(w).ok();
            }
            "2" => {
                present_territory_thread(&mut w, width);
                let saw_both = prompt_continue(&mut w, "See the Da5id thread?");
                if saw_both {
                    present_query_inner_short(&mut w, width);
                    present_connection_punchline(&mut w, width);
                }
                writeln!(w).ok();
                present_end_card(&mut w, width, saw_both);
            }
            _ => {
                present_query_inner(&mut w, width);
                let saw_both = prompt_continue(&mut w, "See the territory collision?");
                if saw_both {
                    present_territory_thread_short(&mut w, width);
                    present_connection_punchline(&mut w, width);
                }
                writeln!(w).ok();
                present_end_card(&mut w, width, saw_both);
            }
        }
    } else {
        present_explore_commands(&mut w);
    }

    if is_tty {
        write!(w, "{SHOW_CURSOR}").ok();
    }
    w.flush().ok();
}

fn prompt_continue(w: &mut impl Write, label: &str) -> bool {
    if !tty() || !io::stdin().is_terminal() {
        return false;
    }
    writeln!(w).ok();
    write!(
        w,
        "  {WHITE}{label} ({BOLD}Enter{RESET}{WHITE}/{BOLD}q{RESET}{WHITE}) {RESET}"
    )
    .ok();
    write!(w, "{SHOW_CURSOR}").ok();
    w.flush().ok();

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).ok();
    let choice = input.trim().to_lowercase();

    write!(w, "{HIDE_CURSOR}").ok();
    w.flush().ok();

    !matches!(choice.as_str(), "q" | "quit" | "n" | "no")
}

fn present_connection_punchline(w: &mut impl Write, width: usize) {
    writeln!(w).ok();
    if tty() {
        rule(w, '\u{2500}', width.min(72));
    }
    writeln!(w).ok();

    let is_tty = tty();
    typewriter(
        w,
        &format!("{BOLD}{WHITE}  The Intel Debrief is where both threads surface.{RESET}"),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(200);
    typewriter(
        w,
        &format!("{BOLD}{WHITE}  Hiro was in 4 of 5 meetings. He never connected the dots.{RESET}"),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(300);
    typewriter(
        w,
        &format!("{BOLD}{WHITE}  Minutes did.{RESET}"),
        if is_tty { 18 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();
}

fn present_explore_commands(w: &mut impl Write) {
    writeln!(w, "  {DIM}Pull the threads:{RESET}").ok();
    writeln!(w).ok();
    writeln!(
        w,
        "    {CYAN}minutes search{RESET} {DIM}\"rendering pipeline\"{RESET}     {DIM}# the Da5id thread{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes search{RESET} {DIM}\"east side\"{RESET}              {DIM}# the territory collision{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes actions{RESET} {DIM}--open{RESET}                  {DIM}# stale commitments{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes people{RESET}                           {DIM}# who knows who{RESET}"
    )
    .ok();
    writeln!(w).ok();
    writeln!(
        w,
        "  {DIM}Or ask your AI:{RESET}  {ITALIC}\"What did Da5id promise and did he deliver?\"{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

// ── Thread 1: The Da5id Problem ─────────────────────────────────

fn present_query_inner(w: &mut impl Write, width: usize) {
    let is_tty = tty();
    let rw = width.min(72);

    writeln!(w).ok();
    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    write!(w, "  {DIM}>{RESET} ").ok();
    typewriter(
        w,
        &format!("{BOLD}{WHITE}What did Da5id promise and did he deliver?{RESET}"),
        if is_tty { 8 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();

    scanning_animation(w);

    pause(150);
    writeln!(
        w,
        "  {GREEN}{BOLD}COMMIT{RESET}   {DIM}Metaverse Infrastructure Standup, 14 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Da5id: \"I think I can fix it by Friday if I swap out{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}the spatial index.\"{RESET}").ok();
    writeln!(w).ok();

    pause(400);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Avatar Expression System, 7 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Juanita: \"Last I heard he was spending time down at the Raft.\"{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(400);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Intel Debrief, yesterday{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Y.T.: \"Uncle Enzo said he saw him at the Raft three times{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}in the last two weeks.\"{RESET}").ok();
    writeln!(w).ok();

    pause(900);

    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Da5id committed to the rendering pipeline fix 14 days ago.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(300);
    typewriter(
        w,
        &format!("{BOLD}{WHITE}  He didn't deliver.{RESET}"),
        if is_tty { 12 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();
    pause(200);

    writeln!(
        w,
        "{DIM}  Minutes connected a casual aside from Juanita, delivery intel"
    )
    .ok();
    writeln!(
        w,
        "  from Y.T., and a 14-day-old commitment from a standup. Three"
    )
    .ok();
    writeln!(
        w,
        "  separate conversations, one thread. No human would have caught it.{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

// ── Thread 2: The Territory Collision ───────────────────────────

fn present_territory_thread(w: &mut impl Write, width: usize) {
    let is_tty = tty();
    let rw = width.min(72);

    writeln!(w).ok();
    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    write!(w, "  {DIM}>{RESET} ").ok();
    typewriter(
        w,
        &format!("{BOLD}{WHITE}Is anything about to disrupt Y.T.'s delivery routes?{RESET}"),
        if is_tty { 8 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();

    scanning_animation(w);

    pause(150);
    writeln!(
        w,
        "  {GREEN}{BOLD}COMMIT{RESET}   {DIM}CosaNostra Delivery Logistics, 11 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Enzo: \"90-day trial. You keep the times under 30, the territory{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}is yours.\"{RESET}").ok();
    writeln!(w).ok();

    pause(400);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Rat Things Perimeter Security, 4 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Ng: \"Anything running through that corridor, deliveries, supply{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}trucks, whatever, is going to hit the new checkpoints.\"{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(400);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Intel Debrief, yesterday{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Y.T.: \"If anyone's doing perimeter changes on the east side,{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}that's going to wreck my routes.\"{RESET}").ok();
    writeln!(w).ok();

    pause(900);

    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Y.T. signed an exclusive Valley delivery territory 11 days ago.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(300);
    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Ng's east-side security expansion is about to reroute her corridor.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();
    pause(200);

    writeln!(
        w,
        "{DIM}  A delivery franchise, a security perimeter, and a passing warning."
    )
    .ok();
    writeln!(
        w,
        "  Three meetings, two teams, zero overlap in the room. Hiro was in"
    )
    .ok();
    writeln!(
        w,
        "  both conversations but never connected the dots.{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

// ── Short variants (skip scanning, used for the second thread) ──

fn present_query_inner_short(w: &mut impl Write, width: usize) {
    let is_tty = tty();
    let rw = width.min(72);

    writeln!(w).ok();
    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    writeln!(
        w,
        "  {DIM}>{RESET} {BOLD}{WHITE}What did Da5id promise and did he deliver?{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(150);
    writeln!(
        w,
        "  {GREEN}{BOLD}COMMIT{RESET}   {DIM}Metaverse Infrastructure Standup, 14 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Da5id: \"I think I can fix it by Friday if I swap out{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}the spatial index.\"{RESET}").ok();
    writeln!(w).ok();

    pause(300);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Avatar Expression System, 7 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Juanita: \"Last I heard he was spending time down at the Raft.\"{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(300);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Intel Debrief, yesterday{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Y.T.: \"Uncle Enzo said he saw him at the Raft three times{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}in the last two weeks.\"{RESET}").ok();
    writeln!(w).ok();

    pause(600);

    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Da5id committed to the rendering pipeline fix 14 days ago.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(300);
    typewriter(
        w,
        &format!("{BOLD}{WHITE}  He didn't deliver.{RESET}"),
        if is_tty { 12 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();
    pause(200);

    writeln!(
        w,
        "{DIM}  Minutes connected a casual aside from Juanita, delivery intel"
    )
    .ok();
    writeln!(
        w,
        "  from Y.T., and a 14-day-old commitment from a standup. Three"
    )
    .ok();
    writeln!(
        w,
        "  separate conversations, one thread. No human would have caught it.{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

fn present_territory_thread_short(w: &mut impl Write, width: usize) {
    let is_tty = tty();
    let rw = width.min(72);

    writeln!(w).ok();
    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    writeln!(
        w,
        "  {DIM}>{RESET} {BOLD}{WHITE}Is anything about to disrupt Y.T.'s delivery routes?{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(150);
    writeln!(
        w,
        "  {GREEN}{BOLD}COMMIT{RESET}   {DIM}CosaNostra Delivery Logistics, 11 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Enzo: \"90-day trial. You keep the times under 30, the territory{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}is yours.\"{RESET}").ok();
    writeln!(w).ok();

    pause(300);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Rat Things Perimeter Security, 4 days ago{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Ng: \"Anything running through that corridor, deliveries, supply{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}trucks, whatever, is going to hit the new checkpoints.\"{RESET}"
    )
    .ok();
    writeln!(w).ok();

    pause(300);

    writeln!(
        w,
        "  {YELLOW}{BOLD}SIGNAL{RESET}   {DIM}Intel Debrief, yesterday{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {ITALIC}Y.T.: \"If anyone's doing perimeter changes on the east side,{RESET}"
    )
    .ok();
    writeln!(w, "    {ITALIC}that's going to wreck my routes.\"{RESET}").ok();
    writeln!(w).ok();

    pause(600);

    if is_tty {
        rule(w, '\u{2500}', rw);
    }
    writeln!(w).ok();

    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Y.T. signed an exclusive Valley delivery territory 11 days ago.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    pause(300);
    typewriter(
        w,
        &format!(
            "{BOLD}{WHITE}  Ng's east-side security expansion is about to reroute her corridor.{RESET}"
        ),
        if is_tty { 10 } else { 0 },
    );
    writeln!(w).ok();
    writeln!(w).ok();
    pause(200);

    writeln!(
        w,
        "{DIM}  A delivery franchise, a security perimeter, and a passing warning."
    )
    .ok();
    writeln!(
        w,
        "  Three meetings, two teams, zero overlap in the room. Hiro was in"
    )
    .ok();
    writeln!(
        w,
        "  both conversations but never connected the dots.{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

// ── End card ────────────────────────────────────────────────────

fn present_end_card(w: &mut impl Write, width: usize, saw_both: bool) {
    if tty() {
        rule(w, '\u{2500}', width.min(72));
    }
    writeln!(w).ok();
    if saw_both {
        writeln!(
            w,
            "  {DIM}Two threads. Five meetings. Zero manual work.{RESET}"
        )
        .ok();
    } else {
        writeln!(
            w,
            "  {DIM}One thread. Five meetings. Zero manual work.{RESET}"
        )
        .ok();
    }
    writeln!(
        w,
        "  {DIM}This is what your AI sees when it reads your conversations.{RESET}"
    )
    .ok();
    writeln!(w).ok();
    writeln!(w, "  {DIM}Try it yourself:{RESET}").ok();
    writeln!(
        w,
        "    {CYAN}minutes search{RESET} {DIM}\"rendering pipeline\"{RESET}     {DIM}# cross-meeting search{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes search{RESET} {DIM}\"east side\"{RESET}              {DIM}# the other thread{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes actions{RESET} {DIM}--open{RESET}                  {DIM}# stale commitments{RESET}"
    )
    .ok();
    writeln!(
        w,
        "    {CYAN}minutes people{RESET}                           {DIM}# who knows who{RESET}"
    )
    .ok();
    writeln!(w).ok();
}

/// Remove all demo meetings from the output directory.
/// Identifies demo files by the `demo` tag in YAML frontmatter, not filename.
/// Returns count of files removed.
pub fn clean_demo_meetings(output_dir: &Path) -> anyhow::Result<usize> {
    let mut removed = 0;
    if !output_dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        // Check frontmatter for demo tag
        if let Ok(content) = fs::read_to_string(&path) {
            if content.starts_with("---") && content.contains(&format!("- {}", DEMO_TAG)) {
                let name = entry.file_name();
                fs::remove_file(&path)?;
                eprintln!("  Removed {}", name.to_string_lossy());
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Run a cross-meeting query against the demo data to show the agent experience
/// without requiring Claude or any MCP client.
pub fn query_demo(output_dir: &Path) -> anyhow::Result<()> {
    let has_demo = output_dir.exists()
        && fs::read_dir(output_dir)?.any(|e| {
            e.ok()
                .and_then(|e| fs::read_to_string(e.path()).ok())
                .is_some_and(|c| c.contains(&format!("- {}", DEMO_TAG)))
        });
    if !has_demo {
        eprintln!("No demo meetings found. Run `minutes demo --full` first.");
        return Ok(());
    }

    let mut w = io::stderr();
    let width = if tty() { term_width() } else { 80 };
    if tty() {
        write!(w, "{HIDE_CURSOR}").ok();
    }

    present_query_inner(&mut w, width);
    writeln!(w).ok();
    present_end_card(&mut w, width, false);

    if tty() {
        write!(w, "{SHOW_CURSOR}").ok();
    }
    w.flush().ok();
    Ok(())
}

/// Generate 5 Snow Crash-themed demo meetings with interconnected storylines.
/// Dates are computed relative to now so they always feel fresh.
/// Returns the paths of created files.
pub fn seed_demo_meetings(output_dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    fs::create_dir_all(output_dir)?;

    let now = Local::now();
    let d = |days_ago: u64, hour: u32, min: u32| -> String {
        let day = now.checked_sub_days(Days::new(days_ago)).unwrap_or(now);
        day.format(&format!("%Y-%m-%dT{:02}:{:02}:00%:z", hour, min))
            .to_string()
    };
    let date_short = |days_ago: u64| -> String {
        let day = now.checked_sub_days(Days::new(days_ago)).unwrap_or(now);
        day.format("%Y-%m-%d").to_string()
    };
    let due_date = |days_ago: u64| -> String {
        let day = now
            .checked_sub_days(Days::new(days_ago.saturating_sub(2)))
            .unwrap_or(now);
        day.format("%Y-%m-%d").to_string()
    };

    let meetings = vec![
        // Meeting 1: Hiro & Da5id — Metaverse Infrastructure Standup
        (
            format!("{}-metaverse-infrastructure-standup.md", date_short(14)),
            format!(
                r#"---
title: Metaverse Infrastructure Standup
type: meeting
date: {date}
duration: 24m
status: complete
tags:
  - {demo}
  - {snowcrash}
attendees:
  - Hiro Protagonist
  - Da5id Meier
people:
  - Hiro Protagonist
  - Da5id Meier
action_items:
  - assignee: Da5id Meier
    task: Fix avatar rendering pipeline before Friday
    due: "{due}"
    status: open
  - assignee: Hiro Protagonist
    task: Benchmark new distributed server architecture
    due: "{due}"
    status: done
decisions:
  - text: Migrate the Street to distributed architecture across 8 regional nodes
    topic: infrastructure
  - text: Deprecate the old monolithic renderer after migration
    topic: infrastructure
speaker_map:
  - speaker_label: SPEAKER_00
    name: Hiro Protagonist
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_01
    name: Da5id Meier
    confidence: high
    source: calendar
---

## Transcript

**Hiro Protagonist**: Da5id, what's the status on the rendering pipeline? We're getting frame drops in the, uh, high-density zones around the Street.

**Da5id Meier**: Yeah so I've been profiling it. The bottleneck is the avatar mesh deduplication layer. When you get more than 10,000 avatars in a sector it just falls off a cliff. I think I can fix it by Friday if I swap out the spatial index.

**Hiro Protagonist**: Friday works. Hold on, let me pull up the... okay yeah. The Black Sun is hosting that concert event next week and we can't have it stuttering with 50k people in there. What about the architecture migration?

**Da5id Meier**: So I looked at your benchmark numbers. The distributed setup is about 3x throughput per node, and we can shard the Street into 8 regions. Latency stays under 40ms for users in the same region. Actually wait, I think it was 38, let me... yeah, 38ms p99.

**Hiro Protagonist**: Let's do it. I'll write up the migration plan. We deprecate the monolith after the new system is stable. No point maintaining both.

**Da5id Meier**: Agreed. I'll have the rendering fix ready by Friday and then I can help with the migration next week. Shouldn't be too bad once the spatial index is swapped.

**Hiro Protagonist**: Sounds good. I already finished the benchmarks by the way. Numbers are in the shared doc. Oh and can you make sure the fix doesn't break the texture streaming? Last time someone touched that layer we had avatars walking around as grey blobs for two hours.

**Da5id Meier**: [laughs] Yeah I remember that. I'll run the full regression suite before I push anything.
"#,
                date = d(14, 10, 0),
                due = due_date(14),
                demo = DEMO_TAG,
                snowcrash = SNOWCRASH_TAG,
            ),
        ),
        // Meeting 2: Y.T. & Uncle Enzo — CosaNostra Delivery Logistics
        (
            format!("{}-cosanostra-delivery-logistics.md", date_short(11)),
            format!(
                r#"---
title: CosaNostra Delivery Logistics
type: meeting
date: {date}
duration: 18m
status: complete
tags:
  - {demo}
  - {snowcrash}
attendees:
  - Y.T.
  - Uncle Enzo
people:
  - Y.T.
  - Uncle Enzo
action_items:
  - assignee: Y.T.
    task: Handle all Valley delivery zone dispatches starting next Monday
    status: open
  - assignee: Uncle Enzo
    task: Send franchise agreement for Y.T. to review
    status: open
decisions:
  - text: "30-minute delivery guarantee is non-negotiable: any violation triggers automatic customer comp"
    topic: operations
  - text: Y.T. gets exclusive Valley territory for 90-day trial period
    topic: territory
speaker_map:
  - speaker_label: SPEAKER_00
    name: Uncle Enzo
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_01
    name: Y.T.
    confidence: high
    source: calendar
---

## Transcript

**Uncle Enzo**: I've been watching your delivery times. You're consistently under 22 minutes in the downtown corridor. That's better than anyone else we've got.

**Y.T.**: My board's dialed in. I know every shortcut between here and the coast. The Valley is where you need help though right?

**Uncle Enzo**: The Valley is a disaster. Three drivers quit last month. Customers are getting cold pizza and the 30-minute guarantee is sacred. You understand? Non-negotiable. A late pizza, we comp the customer automatically. That comes out of the driver's cut.

**Y.T.**: I can handle the Valley. But I want exclusive territory. No other Kouriers poaching my routes or I'm not gonna be able to guarantee anything.

**Uncle Enzo**: 90-day trial. You keep the times under 30, the territory is yours. I'll send over the franchise agreement, read it carefully. My lawyers don't mess around.

**Y.T.**: Deal. When do I start?

**Uncle Enzo**: Monday. And Y.T., one more thing. I heard you've been talking to Hiro Protagonist. He's a friend of ours but keep CosaNostra business out of whatever he's building, capisce?

**Y.T.**: Yeah, understood. Totally separate.

**Uncle Enzo**: Good. [pause] You know, you remind me of myself at your age. Except I didn't have a skateboard. I had a... never mind. Monday. Don't be late.
"#,
                date = d(11, 14, 30),
                demo = DEMO_TAG,
                snowcrash = SNOWCRASH_TAG,
            ),
        ),
        // Meeting 3: Hiro & Juanita — Avatar Expression System
        (
            format!("{}-avatar-expression-system.md", date_short(7)),
            format!(
                r#"---
title: Avatar Expression System
type: meeting
date: {date}
duration: 31m
status: complete
tags:
  - {demo}
  - {snowcrash}
attendees:
  - Hiro Protagonist
  - Juanita Marquez
people:
  - Hiro Protagonist
  - Juanita Marquez
action_items:
  - assignee: Hiro Protagonist
    task: Integrate facial expression SDK into the Metaverse client
    due: "{due}"
    status: open
  - assignee: Juanita Marquez
    task: Send SDK documentation and sample models
    status: done
decisions:
  - text: Ship avatar expressions as a plugin architecture, not baked into core renderer
    topic: architecture
  - text: Use Juanita's 43-muscle facial model as the standard
    topic: technical
speaker_map:
  - speaker_label: SPEAKER_00
    name: Hiro Protagonist
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_01
    name: Juanita Marquez
    confidence: high
    source: calendar
---

## Transcript

**Juanita Marquez**: Okay so I finished the facial expression system. 43 muscle groups, real-time blending. When someone smiles in the Metaverse it actually looks like a smile now, not that weird uncanny valley thing we've been shipping.

**Hiro Protagonist**: That's incredible. The current avatars are basically mannequins. Can I see the... hold on, my mic was muted for like the first ten seconds of this call. Could you hear me?

**Juanita Marquez**: Yeah I heard you fine. Anyway, I'll send you the SDK and sample models after this call. Actually already done, check your messages. The question is how we integrate it. I don't think it should go into the core renderer.

**Hiro Protagonist**: Agreed, especially with Da5id's rendering pipeline already being fragile. Speaking of which, have you talked to him? He was supposed to fix the avatar mesh dedup layer like two weeks ago and I haven't seen a commit.

**Juanita Marquez**: I haven't talked to Da5id in a while honestly. Last I heard he was spending time down at the Raft.

**Hiro Protagonist**: The Raft? He told me he was heads-down on the rendering fix. That's... okay that's not great. Anyway let's make the expression system a plugin. That way it doesn't depend on his pipeline work and we can ship it independently.

**Juanita Marquez**: Smart. Plugin architecture means anyone can build alternative expression systems too. I'll standardize the API. Should have a draft spec by end of week.

**Hiro Protagonist**: Perfect. I'll integrate your SDK by next sprint. And Juanita, seriously, this is going to change how people experience the Metaverse. The emotional bandwidth of a conversation goes way up when you can actually read someone's face.

**Juanita Marquez**: That's literally why I built it. [laughs] Okay I gotta run, I have another call in five.
"#,
                date = d(7, 11, 0),
                due = due_date(7),
                demo = DEMO_TAG,
                snowcrash = SNOWCRASH_TAG,
            ),
        ),
        // Meeting 4: Ng Security Review — Rat Things Perimeter
        (
            format!("{}-rat-things-security-review.md", date_short(4)),
            format!(
                r#"---
title: Rat Things Perimeter Security Review
type: meeting
date: {date}
duration: 42m
status: complete
tags:
  - {demo}
  - {snowcrash}
attendees:
  - Mr. Lee
  - Ng
  - Hiro Protagonist
people:
  - Mr. Lee
  - Ng
  - Hiro Protagonist
action_items:
  - assignee: Ng
    task: Patch east-side perimeter vulnerability with redundant sensor array
    due: "{due}"
    status: open
  - assignee: Mr. Lee
    task: Approve expanded Rat Things budget for additional 12 units
    status: open
  - assignee: Hiro Protagonist
    task: Review autonomous mode failsafe code before deployment
    status: open
decisions:
  - text: Expand Rat Things perimeter coverage by 30% on the east side
    topic: security
  - text: Switch from manual override to autonomous mode for faster response times
    topic: operations
speaker_map:
  - speaker_label: SPEAKER_00
    name: Mr. Lee
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_01
    name: Ng
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_02
    name: Hiro Protagonist
    confidence: high
    source: calendar
---

## Transcript

**Mr. Lee**: Ng, walk us through the perimeter assessment.

**Ng**: So we have a gap on the east side. The current Rat Things deployment covers about 70% of the perimeter there. An intruder came within 50 meters of the compound last Tuesday before a unit intercepted. That's way too close.

**Mr. Lee**: Unacceptable. What do you need?

**Ng**: Twelve additional units and a redundant sensor array. That closes the gap and gives us roughly 30% more coverage. I also recommend switching to autonomous mode. The manual override adds about 4 seconds of response latency and at the speeds these things run, 4 seconds is the difference between interception at 200 meters versus 50.

**Hiro Protagonist**: I wrote the original failsafe code for autonomous mode. It's solid but I should review it before we flip the switch. There's an edge case around the thermal sensors in rain that I've been meaning to look at. Sorry, can you hear that buzzing? I think it's on my end.

**Mr. Lee**: I can hear it. Proceed.

**Hiro Protagonist**: Right, so the edge case. When it rains the thermal signatures blur and the targeting gets... imprecise. I want to make sure the failsafe actually kicks in under those conditions. Give me two days. I want to be thorough.

**Ng**: I can have the new sensor array installed by Wednesday regardless. The Rat Things expansion depends on Mr. Lee's budget approval.

**Mr. Lee**: You'll have it by end of day. Security is not a line item we negotiate on. Ng, proceed with the sensor array. Hiro, get that code review done. Anything else?

**Ng**: One more thing. The units are running hot. Literally. The thermal dissipation on the newer models is... we may need to look at that. But it's a separate issue.

**Mr. Lee**: Flag it. We'll address it next cycle.

**Ng**: Oh one more thing. The expanded perimeter is going to reroute all ground traffic on the east side. Anything running through that corridor, deliveries, supply trucks, whatever, is going to hit the new checkpoints.
"#,
                date = d(4, 9, 0),
                due = due_date(4),
                demo = DEMO_TAG,
                snowcrash = SNOWCRASH_TAG,
            ),
        ),
        // Meeting 5: Hiro & Y.T. — Intel Debrief
        (
            format!("{}-intel-debrief.md", date_short(1)),
            format!(
                r#"---
title: Intel Debrief
type: meeting
date: {date}
duration: 19m
status: complete
tags:
  - {demo}
  - {snowcrash}
attendees:
  - Hiro Protagonist
  - Y.T.
people:
  - Hiro Protagonist
  - Y.T.
action_items:
  - assignee: Hiro Protagonist
    task: Confront Da5id about the rendering pipeline and his whereabouts
    status: open
decisions:
  - text: Postpone Metaverse launch event until rendering pipeline is confirmed fixed
    topic: launch
  - text: Route all Da5id-related commitments through Hiro for accountability
    topic: process
speaker_map:
  - speaker_label: SPEAKER_00
    name: Hiro Protagonist
    confidence: high
    source: calendar
  - speaker_label: SPEAKER_01
    name: Y.T.
    confidence: high
    source: calendar
---

## Transcript

**Hiro Protagonist**: Y.T., thanks for meeting. I need to compare notes on a few things.

**Y.T.**: Yeah sure what's up?

**Hiro Protagonist**: Da5id. Two weeks ago he told me he'd have the rendering pipeline fixed by Friday. That was like 12 days ago. No commit, no update, nothing. And Juanita told me last week she heard he's been spending time at the Raft.

**Y.T.**: Oh that totally tracks. Uncle Enzo mentioned Da5id too actually. Said he saw him at the Raft three times in the last two weeks. Enzo keeps tabs on everyone near the Raft because of the supply chain stuff. He's got people watching.

**Hiro Protagonist**: So wait. Da5id told me he was heads-down coding, but he's actually been at the Raft. The rendering pipeline isn't fixed. The Black Sun concert is in a week and we're going to have 50,000 avatars stuttering at like 4 frames per second. This is bad.

**Y.T.**: Yeah that's not good. Can you fix it yourself?

**Hiro Protagonist**: I can probably hack a workaround but the real fix needs Da5id's expertise on the spatial index. He's the one who wrote that whole layer. I need to talk to him directly and figure out what's going on. In the meantime we should postpone the launch event. I'd rather delay than embarrass ourselves in front of... [sigh] okay yeah we're postponing.

**Y.T.**: Makes sense. Enzo's not going to be happy about the concert delay though. He was planning to use it for a CosaNostra promo. Like a big pizza giveaway thing in the Black Sun.

**Hiro Protagonist**: That's his problem. From now on, anything Da5id commits to goes through me. I need to verify he's actually doing the work before we count on it for planning. I'm not getting burned like this again.

**Y.T.**: Want me to track him down? I know some people at the Raft.

**Hiro Protagonist**: Not yet. Let me try talking to him first. If he ghosts me then... yeah then maybe. Thanks Y.T.

**Y.T.**: Anytime. Let me know how it goes. Oh and heads up, I just signed an exclusive deal for the Valley delivery zone with Enzo. If anyone's doing construction or perimeter changes on the east side, that's going to wreck my routes. Enzo will lose it.

**Hiro Protagonist**: Noted. I'll keep an ear out.
"#,
                date = d(1, 16, 0),
                demo = DEMO_TAG,
                snowcrash = SNOWCRASH_TAG,
            ),
        ),
    ];

    let mut paths = Vec::new();
    for (filename, content) in &meetings {
        let path = output_dir.join(filename);
        if path.exists() {
            continue;
        }
        fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        paths.push(path);
    }

    Ok(paths)
}
