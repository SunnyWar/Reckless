use phf::phf_map;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::BufRead;
use std::sync::Arc;

use crate::{
    board::{Board, NullBoardObserver},
    search::Report,
    thread::{SharedContext, Status, ThreadData},
    threadpool::ThreadPool,
    time::{Limits, TimeManager},
    tools,
    transposition::DEFAULT_TT_SIZE,
    types::{Color, MAX_MOVES, Move, Piece, Score, Square, is_decisive, is_loss, is_win},
};

#[derive(Copy, Clone, PartialEq, Eq)]
enum Mode {
    Cli,
    Uci,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum HandlerResult {
    Continue,
    Quit,
}

struct Settings {
    frc: bool,
    multi_pv: usize,
    move_overhead: u64,
    report: Report,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            frc: false,
            multi_pv: 1,
            move_overhead: 100,
            report: Report::Full,
        }
    }
}

struct HandlerContext<'a> {
    board: &'a mut Board,
    threads: &'a mut ThreadPool,
    settings: &'a mut Settings,
    shared: &'a Arc<SharedContext>,
    mode: &'a mut Mode,
}

impl<'a> HandlerContext<'a> {
    fn is_uci(&self) -> bool {
        matches!(*self.mode, Mode::Uci)
    }
}

type Handler = fn(&mut HandlerContext, &[&str]) -> HandlerResult;

fn handle_uci(ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    uci();
    *ctx.mode = Mode::Uci;
    HandlerResult::Continue
}

fn handle_isready(_ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    println!("readyok");
    HandlerResult::Continue
}

fn handle_go(ctx: &mut HandlerContext, tokens: &[&str]) -> HandlerResult {
    go(ctx.threads, ctx.settings, ctx.board, ctx.shared, tokens);
    HandlerResult::Continue
}

fn handle_position(ctx: &mut HandlerContext, tokens: &[&str]) -> HandlerResult {
    position(ctx.board, ctx.settings, tokens);
    HandlerResult::Continue
}

fn handle_setoption(ctx: &mut HandlerContext, tokens: &[&str]) -> HandlerResult {
    set_option(ctx.threads, ctx.settings, ctx.shared, tokens);
    HandlerResult::Continue
}

fn handle_ucinewgame(ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    reset(ctx.threads, ctx.shared);
    HandlerResult::Continue
}

fn handle_stop(ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    ctx.shared.status.set(Status::STOPPED);
    HandlerResult::Continue
}

fn handle_quit(_ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    HandlerResult::Quit
}

fn handle_compiler(_ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    compiler();
    HandlerResult::Continue
}

fn handle_eval(ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    eval(ctx.threads.main_thread(), ctx.board);
    HandlerResult::Continue
}

fn handle_d(ctx: &mut HandlerContext, _tokens: &[&str]) -> HandlerResult {
    println!("{}", ctx.board);
    HandlerResult::Continue
}

fn handle_bench(ctx: &mut HandlerContext, args: &[&str]) -> HandlerResult {
    if ctx.is_uci() {
        tools::bench::<true>(args);
    } else {
        tools::bench::<false>(args);
    }
    HandlerResult::Continue
}

fn handle_speedtest(_ctx: &mut HandlerContext, args: &[&str]) -> HandlerResult {
    tools::speedtest(args);
    HandlerResult::Continue
}

fn handle_perft(ctx: &mut HandlerContext, args: &[&str]) -> HandlerResult {
    if let Some(depth) = args.first().and_then(|s| s.parse::<usize>().ok()) {
        tools::perft(depth, ctx.board);
    } else {
        eprintln!("Usage: perft <depth>");
    }
    HandlerResult::Continue
}

fn handle_simpleperft(ctx: &mut HandlerContext, args: &[&str]) -> HandlerResult {
    if let Some(depth) = args.first().and_then(|s| s.parse::<usize>().ok()) {
        tools::simple_perft(depth, ctx.board);
    } else {
        eprintln!("Usage: simpleperft <depth>");
    }
    HandlerResult::Continue
}

fn handle_islegalperft(ctx: &mut HandlerContext, args: &[&str]) -> HandlerResult {
    if let Some(depth) = args.first().and_then(|s| s.parse::<usize>().ok()) {
        tools::is_legal_perft(depth, ctx.board);
    } else {
        eprintln!("Usage: islegalperft <depth>");
    }
    HandlerResult::Continue
}

/// All commands: UCI protocol + debugging tools
static COMMANDS: phf::Map<&'static str, Handler> = phf_map! {
    // UCI protocol commands
    "uci" => handle_uci,
    "isready" => handle_isready,
    "go" => handle_go,
    "position" => handle_position,
    "setoption" => handle_setoption,
    "ucinewgame" => handle_ucinewgame,
    "stop" => handle_stop,
    "quit" => handle_quit,
    // Debugging/tool commands
    "compiler" => handle_compiler,
    "eval" => handle_eval,
    "d" => handle_d,
    "bench" => handle_bench,
    "speedtest" => handle_speedtest,
    "perft" => handle_perft,
    "simpleperft" => handle_simpleperft,
    "islegalperft" => handle_islegalperft,
};

pub fn message_loop(mut buffer: VecDeque<String>) {
    let shared = Arc::new(SharedContext::default());
    let mut settings = Settings::default();
    let mut threads = ThreadPool::new(shared.clone());
    let mut board = Board::starting_position();

    let rx = spawn_listener(shared.clone());

    let mut mode = if buffer.is_empty() { Mode::Uci } else { Mode::Cli };

    loop {
        let message = if let Some(cmd) = buffer.pop_front() {
            cmd
        } else if mode == Mode::Uci {
            match rx.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            }
        } else {
            break;
        };

        let tokens = message.split_whitespace().collect::<Vec<_>>();

        // Skip empty lines
        if !tokens.is_empty() {
            let cmd = tokens[0];
            let args = &tokens[1..];

            // Try to dispatch command
            let handler = COMMANDS.get(cmd);

            if let Some(handler) = handler {
                let mut ctx = HandlerContext {
                    board: &mut board,
                    threads: &mut threads,
                    settings: &mut settings,
                    shared: &shared,
                    mode: &mut mode,
                };

                match handler(&mut ctx, args) {
                    HandlerResult::Continue => {}
                    HandlerResult::Quit => break,
                }
            } else {
                eprintln!("Unknown command: '{}'", message.trim_end());
            }
        }

        // Auto-exit after last CLI command
        if matches!(mode, Mode::Cli) && buffer.is_empty() {
            break;
        }
    }
}

fn spawn_listener(shared: Arc<SharedContext>) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line_result in stdin.lock().lines() {
            let message = match line_result {
                Ok(msg) => msg,
                Err(_) => {
                    // EOF or error: send quit if not running
                    if shared.status.get() != Status::RUNNING {
                        let _ = tx.send("quit".into());
                    }
                    break;
                }
            };

            match message.trim_end() {
                "isready" => println!("readyok"),
                "stop" => shared.status.set(Status::STOPPED),
                "quit" => {
                    shared.status.set(Status::STOPPED);
                    let _ = tx.send("quit".into());
                    break;
                }
                _ => {
                    // According to the UCI specs, commands that are unexpected
                    // in the current state should be ignored silently.
                    // (https://backscattering.de/chess/uci/#unexpected)
                    if shared.status.get() != Status::RUNNING {
                        let _ = tx.send(message);
                    }
                }
            }
        }
    });

    rx
}

fn uci() {
    println!("id name Reckless {}", env!("ENGINE_VERSION"));
    println!("id author Arseniy Surkov, Shahin M. Shahin, and Styx");
    println!("option name Hash type spin default {DEFAULT_TT_SIZE} min 1 max 262144");
    println!("option name Threads type spin default 1 min 1 max {}", ThreadPool::available_threads());
    println!("option name MoveOverhead type spin default 100 min 0 max 2000");
    println!("option name Minimal type check default false");
    println!("option name Clear Hash type button");
    println!("option name UCI_Chess960 type check default false");
    println!("option name MultiPV type spin default 1 min 1 max {MAX_MOVES}");

    #[cfg(feature = "syzygy")]
    println!("option name SyzygyPath type string default");

    #[cfg(feature = "spsa")]
    crate::parameters::print_options();

    println!("uciok");
}

fn compiler() {
    println!("Compiler Version: {}", env!("COMPILER_VERSION"));
    println!("Compiler Target: {}", env!("COMPILER_TARGET"));
    println!("Compiler Features: {}", env!("COMPILER_FEATURES"));
}

fn reset(threads: &mut ThreadPool, shared: &Arc<SharedContext>) {
    threads.clear();
    shared.tt.clear(threads.len());

    for corrhist in shared.history.all() {
        corrhist.pawn.clear();
        corrhist.non_pawn[Color::White].clear();
        corrhist.non_pawn[Color::Black].clear();
    }
}

impl ThreadData {
    fn vote_value(&self, min_score: i32) -> i32 {
        (self.root_moves[0].score - min_score + 10) * self.completed_depth
    }
}

fn compute_votes(threads: &ThreadPool, min_score: i32) -> HashMap<Move, i32> {
    let mut votes: HashMap<Move, i32> = HashMap::new();
    for result in threads.iter() {
        *votes.entry(result.root_moves[0].mv).or_default() += result.vote_value(min_score);
    }
    votes
}

fn select_best_thread(threads: &ThreadPool, votes: &HashMap<Move, i32>, min_score: i32) -> usize {
    let mut best = 0;

    if !matches!(threads[best].time_manager.limits(), Limits::Depth(_)) && threads[0].multi_pv == 1 {
        for current in 1..threads.len() {
            let is_better_candidate = || -> bool {
                let best_td = &threads[best];
                let current_td = &threads[current];

                if is_win(best_td.root_moves[0].score) {
                    return current_td.root_moves[0].score > best_td.root_moves[0].score;
                }

                if current_td.root_moves[0].score != -Score::INFINITE
                    && best_td.root_moves[0].score != -Score::INFINITE
                    && is_loss(best_td.root_moves[0].score)
                {
                    return current_td.root_moves[0].score < best_td.root_moves[0].score;
                }

                if current_td.root_moves[0].score != -Score::INFINITE && is_decisive(current_td.root_moves[0].score) {
                    return true;
                }

                let best_vote = votes[&best_td.root_moves[0].mv];
                let current_vote = votes[&current_td.root_moves[0].mv];

                !is_loss(current_td.root_moves[0].score)
                    && (current_vote > best_vote
                        || (current_vote == best_vote
                            && current_td.vote_value(min_score) > best_td.vote_value(min_score)))
            };

            if is_better_candidate() {
                best = current;
            }
        }
    }

    best
}

fn go(threads: &mut ThreadPool, settings: &Settings, board: &Board, shared: &Arc<SharedContext>, tokens: &[&str]) {
    let limits = parse_limits(board.side_to_move(), tokens);
    let time_manager = TimeManager::new(limits, board.fullmove_number(), settings.move_overhead);

    threads.execute_searches(time_manager, settings.report, settings.multi_pv, board, shared);

    if threads[0].root_moves.is_empty() {
        println!("bestmove (none)");
        return;
    }

    let min_score = threads.iter().map(|v| v.root_moves[0].score).min().unwrap();
    let votes = compute_votes(threads, min_score);
    let best = select_best_thread(threads, &votes, min_score);

    if best != 0 {
        threads[best].print_uci_info(threads[best].completed_depth);
    }

    println!("bestmove {}", threads[best].root_moves[0].mv.to_uci(board));
    crate::misc::dbg_print();
}

fn position(board: &mut Board, settings: &Settings, tokens: &[&str]) {
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "startpos" => {
                *board = Board::starting_position();
                i += 1;
            }
            "fen" => {
                let fen_tokens = &tokens[i + 1..];
                let end = fen_tokens.iter().position(|&t| t == "moves").unwrap_or(fen_tokens.len());
                let fen = fen_tokens[..end].join(" ");
                match Board::from_fen(&fen) {
                    Ok(b) => *board = b,
                    Err(e) => eprintln!("Invalid FEN: {e:?}"),
                }
                board.set_frc(settings.frc);
                // Skip past FEN tokens
                i += 1 + end;
            }
            "moves" => {
                for uci_move in &tokens[i + 1..] {
                    make_uci_move(board, uci_move);
                }
                break;
            }
            _ => i += 1,
        }
    }
}

fn make_uci_move(board: &mut Board, uci_move: &str) {
    let moves = board.generate_all_moves();
    if let Some(entry) = moves.iter().find(|e| e.mv.to_uci(board) == uci_move) {
        board.make_move(entry.mv, &mut NullBoardObserver);
        board.advance_fullmove_counter();
    }
}

fn set_option(threads: &mut ThreadPool, settings: &mut Settings, shared: &Arc<SharedContext>, tokens: &[&str]) {
    match tokens {
        ["name", "Minimal", "value", v] => match *v {
            "true" => settings.report = Report::Minimal,
            "false" => settings.report = Report::Full,
            _ => eprintln!("Invalid value: '{v}'"),
        },
        ["name", "Clear", "Hash"] => {
            shared.tt.clear(threads.len());
            println!("info string Hash cleared");
        }
        ["name", "Hash", "value", v] => {
            shared.tt.resize(threads.len(), v.parse().unwrap());
            println!("info string set Hash to {v} MB");
        }
        ["name", "Threads", "value", v] => {
            threads.set_count(v.parse().unwrap_or(1));
            println!("info string set Threads to {}", threads.len());
        }
        ["name", "MoveOverhead", "value", v] => {
            settings.move_overhead = v.parse().unwrap();
            println!("info string set MoveOverhead to {v} ms");
        }
        #[cfg(feature = "syzygy")]
        ["name", "SyzygyPath", "value", v] => match crate::tb::initialize(v) {
            Some(size) => println!("info string Loaded Syzygy tablebases with {size} pieces"),
            None => eprintln!("Failed to load Syzygy tablebases"),
        },
        ["name", "UCI_Chess960", "value", v] => {
            settings.frc = v.parse().unwrap_or_default();
            println!("info string set UCI_Chess960 to {v}");
        }
        ["name", "MultiPV", "value", v] => {
            settings.multi_pv = v.parse().unwrap_or_default();
            println!("info string set MultiPV to {v}");
        }
        #[cfg(feature = "spsa")]
        ["name", name, "value", v] => {
            crate::parameters::set_parameter(name, v);
            println!("info string set {name} to {v}");
        }
        _ => eprintln!("Unknown option: '{}'", tokens.join(" ").trim_end()),
    }
}

fn eval(td: &mut ThreadData, board: &Board) {
    td.nnue.full_refresh(board);
    td.nnue.evaluate(board);

    let side = board.side_to_move();

    println!("NNUE derived piece values");
    println!("+-------+-------+-------+-------+-------+-------+-------+-------+");
    for rank in (0..8).rev() {
        print!("|");
        for file in 0..8 {
            let sq = Square::from_rank_file(rank, file);
            let piece = board.piece_on(sq);
            let piece_str = if piece == Piece::None { " ".to_string() } else { piece.to_string() };
            print!("  {:^3}  |", piece_str);
        }
        println!();

        print!("|");
        for file in 0..8 {
            let sq = Square::from_rank_file(rank, file);
            match td.nnue.piece_contribution(board, sq) {
                None => print!("       |"),
                Some(v) => {
                    let val = v as f32 / 100.0;
                    print!("{:+6.2} |", val);
                }
            }
        }
        println!();
        println!("+-------+-------+-------+-------+-------+-------+-------+-------+");
    }

    let used_bucket = crate::nnue::OUTPUT_BUCKETS_LAYOUT[board.occupancies().popcount()];

    println!("\nNNUE output buckets (White side)");
    println!("+------------+------------+");
    println!("|   Bucket   |   Total    |");
    println!("+------------+------------+");

    for bucket in 0..8 {
        let raw_score = td.nnue.eval_with_bucket(board, bucket);
        let white_score = if side == Color::White { raw_score } else { -raw_score };
        let total = white_score as f32 / 100.0;

        if bucket == used_bucket {
            println!("|  {:<2}        | {:+7.2}    | <-- this bucket is used", bucket, total);
        } else {
            println!("|  {:<2}        | {:+7.2}    |", bucket, total);
        }
    }
    println!("+------------+------------+");

    let final_eval = td.nnue.evaluate(board);
    let final_total = (if side == Color::White { final_eval } else { -final_eval }) as f32 / 100.0;
    println!("\nNNUE evaluation        {:+.2} (White side)", final_total);
}

fn parse_limits(color: Color, tokens: &[&str]) -> Limits {
    if let ["infinite"] = tokens {
        return Limits::Infinite;
    }

    let mut main = None;
    let mut inc = None;
    let mut moves = None;

    for chunk in tokens.chunks(2) {
        if let [name, value] = *chunk {
            let Ok(value) = value.parse::<u64>() else {
                continue;
            };

            match name {
                "depth" if value > 0 => return Limits::Depth(value as i32),
                "movetime" if value > 0 => return Limits::Time(value),
                "nodes" if value > 0 => return Limits::Nodes(value),
                _ => {}
            }

            match name {
                "wtime" if Color::White == color => main = Some(value),
                "btime" if Color::Black == color => main = Some(value),
                "winc" if Color::White == color => inc = Some(value),
                "binc" if Color::Black == color => inc = Some(value),
                "movestogo" => moves = Some(value),
                _ => continue,
            }
        }
    }

    if main.is_none() && inc.is_none() {
        return Limits::Infinite;
    }

    let main = main.unwrap_or_default();
    let inc = inc.unwrap_or_default();

    match moves {
        Some(moves) => Limits::Cyclic(main, inc, moves),
        None => Limits::Fischer(main, inc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_position_helper(tokens: &[&str]) -> Board {
        let settings = Settings::default();
        let mut board = Board::starting_position();

        position(&mut board, &settings, tokens);
        board.clone()
    }

    #[test]
    fn test_position_startpos() {
        let board = test_position_helper(&["startpos"]);
        assert_eq!(board.to_fen(), "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1");
        let board = test_position_helper(&[]);
        assert_eq!(board.to_fen(), "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1");
    }

    #[test]
    fn test_position_startpos_multiple_moves() {
        let board = test_position_helper(&["moves", "e2e4", "e7e5", "g1f3"]);
        assert_eq!(board.side_to_move(), Color::Black);
        let fen = board.to_fen();
        let fen_position = fen.split_whitespace().next().unwrap();
        assert!(fen_position.contains("5N2"));
    }

    #[test]
    fn test_position_fen_with_moves() {
        let board = test_position_helper(&[
            "fen",
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR",
            "b",
            "KQkq",
            "e3",
            "0",
            "1",
            "moves",
            "e7e5",
        ]);
        assert_eq!(board.side_to_move(), Color::White);
    }

    #[test]
    fn test_position_empty_moves_list() {
        let board = test_position_helper(&["moves"]);
        assert_eq!(board.to_fen(), "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1");
    }

    #[test]
    fn test_position_invalid_move_ignored() {
        let board = test_position_helper(&["moves", "e2e4", "invalid", "e7e5"]);
        assert_eq!(board.side_to_move(), Color::White);
    }

    #[test]
    fn test_position_long_move_sequence() {
        let board = test_position_helper(&["moves", "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"]);
        assert_eq!(board.side_to_move(), Color::White);
    }

    #[test]
    fn test_position_castling() {
        let board = test_position_helper(&[
            "fen",
            "r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R",
            "w",
            "KQkq",
            "-",
            "0",
            "1",
            "moves",
            "e1g1",
        ]);
        assert_eq!(board.side_to_move(), Color::Black);
    }

    #[test]
    fn test_position_en_passant() {
        let board = test_position_helper(&[
            "fen",
            "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR",
            "w",
            "KQkq",
            "f6",
            "0",
            "1",
            "moves",
            "e5f6",
        ]);
        assert_eq!(board.side_to_move(), Color::Black);
    }

    #[test]
    fn test_position_promotion() {
        let board = test_position_helper(&["fen", "8/P7/8/8/8/8/8/4K2k", "w", "-", "-", "0", "1", "moves", "a7a8q"]);
        assert_eq!(board.side_to_move(), Color::Black);
    }

    #[test]
    fn test_make_uci_move_invalid() {
        let mut board = Board::starting_position();
        let fen_before = board.to_fen();
        make_uci_move(&mut board, "invalid_move");
        assert_eq!(board.to_fen(), fen_before);
    }

    #[test]
    fn test_position_moves_without_startpos_ignored() {
        let board = test_position_helper(&["moves", "e2e4", "e7e5"]);
        assert_eq!(board.to_fen(), "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2");
    }
}
