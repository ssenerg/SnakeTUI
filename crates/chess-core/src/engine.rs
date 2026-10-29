//! The computer opponent: an alpha-beta search over cozy-chess's board at
//! five [`LEVELS`], from one that often blunders to one that looks as deep
//! as its budget allows. Every move searches a bounded number of positions,
//! so it takes a few milliseconds, and the same game, level and seed always
//! give the same move.

use std::cmp::Reverse;
use std::ops::RangeInclusive;

use cozy_chess::{BitBoard, Board, Color as Side, File, Piece as P, Rank};
use rand::{Rng, RngExt};

use crate::game::Game;
use crate::moves::Move;
use crate::piece::Piece;
use crate::position::{Position, repetition_key};
use crate::square::Square;

/// How strong the computer plays: 1 is the weakest.
pub const LEVELS: RangeInclusive<u8> = 1..=5;

/// The most positions one move looks at, at any level, which is what keeps
/// it quick.
const NODES: u64 = 50_000;

/// The longest line searched, checks and captures included.
const MAX_PLY: usize = 64;

const INFINITY: i32 = 32_000;

/// The score for giving mate now. A mate `n` plies off scores `MATE - n`,
/// so the search mates as soon as it can and holds out when it can't.
const MATE: i32 = 31_000;

/// Scores past this are mates.
const MATE_BOUND: i32 = MATE - MAX_PLY as i32;

/// Positions the search remembers; a power of two.
const TABLE_SIZE: usize = 1 << 14;

/// Chooses a move for the side to move at `level`, which is clamped to
/// [`LEVELS`]; `None` only when there's no legal move. It knows nothing of
/// how the game got here; [`choose_in`] does.
pub fn choose<R: Rng + ?Sized>(position: &Position, level: u8, rng: &mut R) -> Option<Move> {
    choose_after(position, &[], level, rng)
}

/// Chooses the next move in `game` as [`choose`] does, knowing the
/// positions it has been through: the computer steers clear of a third
/// repetition when it's ahead, and makes for one when it's behind.
pub fn choose_in<R: Rng + ?Sized>(game: &Game, level: u8, rng: &mut R) -> Option<Move> {
    let before = game
        .seen()
        .split_last()
        .map_or(&[][..], |(_, before)| before);
    choose_after(game.position(), before, level, rng)
}

/// Chooses a move in `position`, which came after the positions with the
/// repetition keys `before` since the last capture or pawn move.
fn choose_after<R: Rng + ?Sized>(
    position: &Position,
    before: &[u64],
    level: u8,
    rng: &mut R,
) -> Option<Move> {
    let board = position.cozy();
    let style = Style::of(level);
    let mut moves = Vec::with_capacity(64);
    board.generate_moves(|piece_moves| {
        moves.extend(piece_moves);
        false
    });
    if moves.is_empty() {
        return None;
    }
    if style.random > 0 && rng.random_ratio(1, style.random) {
        let mv = moves[rng.random_range(0..moves.len())];
        return Some(from_cozy(board, mv));
    }
    let mut roots: Vec<Root> = moves
        .into_iter()
        .map(|mv| Root {
            mv,
            noise: rng.random_range(-style.noise..=style.noise),
            score: 0,
        })
        .collect();
    let mut search = Search::new(style.quiesce, before);
    let best = search.root(board, &mut roots, style.depth);
    Some(from_cozy(board, best))
}

/// How one level plays.
struct Style {
    /// Plies searched before only captures are; checks add one.
    depth: usize,
    /// Whether to play out captures past `depth`.
    quiesce: bool,
    /// Each move's score is moved up or down by up to this many centipawns.
    noise: i32,
    /// One move in this many is any legal move at all; 0 for never.
    random: u32,
}

impl Style {
    fn of(level: u8) -> Style {
        let (depth, quiesce, noise, random) = match level.clamp(*LEVELS.start(), *LEVELS.end()) {
            1 => (1, false, 300, 4),
            2 => (1, true, 120, 0),
            3 => (2, true, 50, 0),
            4 => (3, true, 15, 0),
            _ => (MAX_PLY / 2, true, 0, 0),
        };
        Style {
            depth,
            quiesce,
            noise,
            random,
        }
    }
}

/// A move from the position being played, with its noise and last score.
struct Root {
    mv: cozy_chess::Move,
    noise: i32,
    score: i32,
}

/// A move waiting to be searched; higher scores go first.
#[derive(Clone, Copy)]
struct Ordered {
    mv: cozy_chess::Move,
    score: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Exact,
    /// At least the score: a move was too good for the other side to allow.
    Lower,
    /// At most the score: nothing was good enough.
    Upper,
}

/// What a finished search found about a position.
#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    mv: Option<cozy_chess::Move>,
    score: i32,
    depth: u8,
    bound: Bound,
}

struct Search<'a> {
    quiesce: bool,
    /// The game's positions before the root since the last capture or
    /// pawn move, by repetition key.
    before: &'a [u64],
    nodes: u64,
    /// Set once the budget's spent; every score after it is thrown away.
    stopped: bool,
    /// Every node's moves on the current line, each after its parent's.
    moves: Vec<Ordered>,
    table: Vec<Option<Entry>>,
    /// Quiet moves that refuted something at each ply, tried early there.
    killers: [[Option<cozy_chess::Move>; 2]; MAX_PLY],
    /// How often a quiet move of a piece to a square refuted something.
    history: [[i32; 64]; 6],
    /// The positions on the current line by repetition key.
    path: [u64; MAX_PLY + 1],
}

impl<'a> Search<'a> {
    fn new(quiesce: bool, before: &'a [u64]) -> Search<'a> {
        Search {
            quiesce,
            before,
            nodes: 0,
            stopped: false,
            moves: Vec::with_capacity(512),
            table: vec![None; TABLE_SIZE],
            killers: [[None; 2]; MAX_PLY],
            history: [[0; 64]; 6],
            path: [0; MAX_PLY + 1],
        }
    }

    /// The best of `roots` after searching one ply deeper each time, up to
    /// `depth`, each move's score moved by its noise. A search the budget
    /// cut short counts for the moves it finished.
    fn root(&mut self, board: &Board, roots: &mut [Root], depth: usize) -> cozy_chess::Move {
        self.path[0] = repetition_key(board);
        for root in roots.iter_mut() {
            root.score = self.order(board, root.mv, 0, None);
        }
        roots.sort_by_key(|root| Reverse(root.score));
        let mut best = roots[0].mv;
        for depth in 1..=depth {
            let mut alpha = -INFINITY;
            let mut found = None;
            for (i, root) in roots.iter_mut().enumerate() {
                let mut next = board.clone();
                next.play_unchecked(root.mv);
                // Searched for whether it beats alpha once its noise is added.
                let score = -self.negamax(
                    &next,
                    depth - 1,
                    1,
                    root.noise - INFINITY,
                    root.noise - alpha,
                ) + root.noise;
                if self.stopped {
                    break;
                }
                root.score = score;
                if score > alpha {
                    alpha = score;
                    found = Some(i);
                }
            }
            let Some(i) = found else { break };
            best = roots[i].mv;
            // A mate either way won't change with more depth.
            let mate = (roots[i].score - roots[i].noise).abs() > MATE_BOUND;
            if self.stopped || mate {
                break;
            }
            roots.sort_by_key(|root| Reverse(root.score));
        }
        best
    }

    /// The score of `board` for the side to move, searching `depth` plies
    /// for one between `alpha` and `beta`. Outside them, it's only a bound.
    fn negamax(
        &mut self,
        board: &Board,
        depth: usize,
        ply: usize,
        mut alpha: i32,
        beta: i32,
    ) -> i32 {
        let repetition = repetition_key(board);
        if self.repeats(repetition, board.halfmove_clock(), ply) || insufficient(board) {
            return 0;
        }
        let in_check = !board.checkers().is_empty();
        let depth = depth + usize::from(in_check);
        if depth == 0 || ply >= MAX_PLY {
            return if self.quiesce {
                self.quiescence(board, ply, alpha, beta)
            } else {
                self.leaf(board, ply)
            };
        }
        if self.spend() {
            return 0;
        }

        let key = board.hash();
        let mut best_move = None;
        if let Some(entry) = self.probe(key) {
            best_move = entry.mv;
            if usize::from(entry.depth) >= depth {
                let score = from_table(entry.score, ply);
                let enough = match entry.bound {
                    Bound::Exact => true,
                    Bound::Lower => score >= beta,
                    Bound::Upper => score <= alpha,
                };
                if enough {
                    return score;
                }
            }
        }

        let start = self.moves.len();
        self.generate(board, ply, best_move, false);
        let end = self.moves.len();
        if start == end {
            return if in_check { ply as i32 - MATE } else { 0 };
        }
        if board.halfmove_clock() >= 100 {
            self.moves.truncate(start);
            return 0;
        }

        self.path[ply] = repetition;
        let alpha_before = alpha;
        let mut best = -INFINITY;
        for i in start..end {
            let mv = self.pick(i, end);
            let mut next = board.clone();
            next.play_unchecked(mv);
            let score = -self.negamax(&next, depth - 1, ply + 1, -beta, -alpha);
            if self.stopped {
                break;
            }
            if score > best {
                best = score;
                best_move = Some(mv);
                if score > alpha {
                    alpha = score;
                    if score >= beta {
                        self.refuted(board, mv, depth, ply);
                        break;
                    }
                }
            }
        }
        self.moves.truncate(start);
        if self.stopped {
            return 0;
        }

        let bound = if best >= beta {
            Bound::Lower
        } else if best > alpha_before {
            Bound::Exact
        } else {
            Bound::Upper
        };
        self.table[key as usize % TABLE_SIZE] = Some(Entry {
            key,
            mv: best_move,
            score: to_table(best, ply),
            depth: depth.min(u8::MAX.into()) as u8,
            bound,
        });
        best
    }

    /// The score of `board` once the captures on it are played out, so no
    /// line stops halfway through an exchange. The side to move may stand
    /// pat rather than capture, unless it's in check.
    fn quiescence(&mut self, board: &Board, ply: usize, mut alpha: i32, beta: i32) -> i32 {
        if self.spend() {
            return 0;
        }
        if ply >= MAX_PLY {
            return evaluate(board);
        }
        let in_check = !board.checkers().is_empty();
        let mut best = -INFINITY;
        if !in_check {
            best = evaluate(board);
            if best >= beta {
                return best;
            }
            alpha = alpha.max(best);
        }

        let start = self.moves.len();
        self.generate(board, ply, None, !in_check);
        let end = self.moves.len();
        if in_check && start == end {
            return ply as i32 - MATE;
        }
        for i in start..end {
            let mv = self.pick(i, end);
            let mut next = board.clone();
            next.play_unchecked(mv);
            let score = -self.quiescence(&next, ply + 1, -beta, -alpha);
            if self.stopped {
                break;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    if score >= beta {
                        break;
                    }
                }
            }
        }
        self.moves.truncate(start);
        best
    }

    /// The score of `board` without searching further, mate and stalemate
    /// aside.
    fn leaf(&mut self, board: &Board, ply: usize) -> i32 {
        if self.spend() {
            return 0;
        }
        if board.generate_moves(|_| true) {
            evaluate(board)
        } else if board.checkers().is_empty() {
            0
        } else {
            ply as i32 - MATE
        }
    }

    /// Counts a position looked at, and stops the search once the budget's
    /// spent.
    fn spend(&mut self) -> bool {
        self.nodes += 1;
        if self.nodes > NODES {
            self.stopped = true;
        }
        self.stopped
    }

    /// Whether the position with repetition key `key` is a draw as far as
    /// the search cares: it stood earlier on this line past the root, as
    /// what was repeated once can be again, or it's the position's third
    /// time in the game. Nothing before the last capture or pawn move
    /// counts.
    fn repeats(&self, key: u64, halfmove_clock: u8, ply: usize) -> bool {
        let reach = usize::from(halfmove_clock);
        let on_line = (2..=reach.min(ply - 1))
            .step_by(2)
            .any(|back| self.path[ply - back] == key);
        if on_line {
            return true;
        }
        if reach < ply {
            return false;
        }
        let in_game = std::iter::once(&self.path[0]).chain(self.before);
        in_game.filter(|&&earlier| earlier == key).nth(1).is_some()
    }

    fn probe(&self, key: u64) -> Option<Entry> {
        self.table[key as usize % TABLE_SIZE].filter(|entry| entry.key == key)
    }

    /// Adds the moves from `board` to search, each scored by how early to
    /// try it; only captures and queen promotions when `captures`.
    fn generate(
        &mut self,
        board: &Board,
        ply: usize,
        best: Option<cozy_chess::Move>,
        captures: bool,
    ) {
        let us = board.side_to_move();
        let them = board.colors(!us);
        let en_passant = board.en_passant().map_or(BitBoard::EMPTY, |file| {
            cozy_chess::Square::new(file, Rank::Sixth.relative_to(us)).bitboard()
        });
        let last_rank = Rank::Eighth.relative_to(us).bitboard();
        board.generate_moves(|mut moves| {
            if captures {
                moves.to &= match moves.piece {
                    P::Pawn => them | en_passant | last_rank,
                    _ => them,
                };
            }
            for mv in moves {
                if captures && mv.promotion.is_some_and(|piece| piece != P::Queen) {
                    continue;
                }
                let score = self.order(board, mv, ply, best);
                self.moves.push(Ordered { mv, score });
            }
            false
        });
    }

    /// How early to try `mv`: the best move found before, then captures of
    /// the most valuable piece by the least valuable one, then quiet moves
    /// that refuted others, with underpromotions last.
    fn order(
        &self,
        board: &Board,
        mv: cozy_chess::Move,
        ply: usize,
        best: Option<cozy_chess::Move>,
    ) -> i32 {
        if best == Some(mv) {
            return 1_000_000;
        }
        let victim = captured(board, mv).map_or(0, value);
        match mv.promotion {
            Some(P::Queen) => return 100_000 + 10 * (victim + value(P::Queen)),
            Some(_) => return victim - 100_000,
            None => {}
        }
        let mover = board.piece_on(mv.from).unwrap_or(P::Pawn);
        if victim > 0 {
            100_000 + 10 * victim - value(mover)
        } else if self.killers[ply][0] == Some(mv) {
            90_000
        } else if self.killers[ply][1] == Some(mv) {
            89_000
        } else {
            self.history[mover as usize][mv.to as usize]
        }
    }

    /// Moves the highest scored of the moves from `i` to `end` to `i`, and
    /// returns it.
    fn pick(&mut self, i: usize, end: usize) -> cozy_chess::Move {
        let mut best = i;
        for j in i + 1..end {
            if self.moves[j].score > self.moves[best].score {
                best = j;
            }
        }
        self.moves.swap(i, best);
        self.moves[i].mv
    }

    /// Remembers a quiet move that refuted the line, to try it early in
    /// its neighbours.
    fn refuted(&mut self, board: &Board, mv: cozy_chess::Move, depth: usize, ply: usize) {
        if captured(board, mv).is_some() || mv.promotion.is_some() {
            return;
        }
        let killers = &mut self.killers[ply];
        if killers[0] != Some(mv) {
            killers[1] = killers[0];
            killers[0] = Some(mv);
        }
        let piece = board.piece_on(mv.from).unwrap_or(P::Pawn);
        let history = &mut self.history[piece as usize][mv.to as usize];
        *history = (*history + (depth * depth) as i32).min(80_000);
    }
}

/// The table keeps mates counted from the position, which may turn up at
/// another ply; the search counts them from the root.
fn to_table(score: i32, ply: usize) -> i32 {
    match score {
        _ if score > MATE_BOUND => score + ply as i32,
        _ if score < -MATE_BOUND => score - ply as i32,
        _ => score,
    }
}

fn from_table(score: i32, ply: usize) -> i32 {
    match score {
        _ if score > MATE_BOUND => score - ply as i32,
        _ if score < -MATE_BOUND => score + ply as i32,
        _ => score,
    }
}

/// What `mv` takes, en passant included.
fn captured(board: &Board, mv: cozy_chess::Move) -> Option<P> {
    if board.colors(!board.side_to_move()).has(mv.to) {
        return board.piece_on(mv.to);
    }
    let pawn = board.piece_on(mv.from) == Some(P::Pawn);
    (pawn && mv.from.file() != mv.to.file()).then_some(P::Pawn)
}

/// `mv` as chess-core writes it: castling, which cozy-chess plays as the
/// king taking its own rook, is the king's two-square step.
fn from_cozy(board: &Board, mv: cozy_chess::Move) -> Move {
    let castles =
        board.piece_on(mv.from) == Some(P::King) && board.colors(board.side_to_move()).has(mv.to);
    let to = if castles {
        let file = if mv.to.file() > mv.from.file() {
            File::G
        } else {
            File::C
        };
        cozy_chess::Square::new(file, mv.from.rank())
    } else {
        mv.to
    };
    Move {
        from: Square::from_cozy(mv.from),
        to: Square::from_cozy(to),
        promotion: mv.promotion.map(Piece::from_cozy),
    }
}

/// Whether neither side has enough left to mate: no pawns, rooks or queens,
/// and at most one knight or bishop.
fn insufficient(board: &Board) -> bool {
    let heavy = board.pieces(P::Pawn) | board.pieces(P::Rook) | board.pieces(P::Queen);
    let minors = board.pieces(P::Knight) | board.pieces(P::Bishop);
    heavy.is_empty() && minors.len() <= 1
}

/// A piece's worth in centipawns; the king's is never at stake.
fn value(piece: P) -> i32 {
    [100, 320, 330, 500, 900, 0][piece as usize]
}

/// How good `board` is for the side to move, in centipawns: the material,
/// where each piece stands, and for the king, how much is left besides.
fn evaluate(board: &Board) -> i32 {
    if insufficient(board) {
        return 0;
    }
    let phase = phase(board);
    let mut score = 0;
    for side in Side::ALL {
        // The tables are written for white from the eighth rank down.
        let (sign, flip) = match side {
            Side::White => (1, 56),
            Side::Black => (-1, 0),
        };
        for piece in P::ALL {
            for square in board.colored_pieces(side, piece) {
                let i = square as usize ^ flip;
                let placed = match piece {
                    P::King => (KING_MIDDLE[i] * phase + KING_END[i] * (24 - phase)) / 24,
                    _ => SQUARES[piece as usize][i],
                };
                score += sign * (value(piece) + placed);
            }
        }
    }
    score += mop_up(board);
    match board.side_to_move() {
        Side::White => score,
        Side::Black => -score,
    }
}

/// How far from the ending: 24 with every knight, bishop, rook and queen
/// on the board, 0 with none.
fn phase(board: &Board) -> i32 {
    let minors = (board.pieces(P::Knight) | board.pieces(P::Bishop)).len();
    let phase = minors + 2 * board.pieces(P::Rook).len() + 4 * board.pieces(P::Queen).len();
    phase.min(24) as i32
}

/// Against a lone king, white's bonus (or black's, negative) for pushing it
/// to the edge and bringing its own king close, which is how a mate without
/// pawns is given.
fn mop_up(board: &Board) -> i32 {
    for (winner, sign) in [(Side::White, 1), (Side::Black, -1)] {
        let loser = !winner;
        let pieces = board.colors(winner) & !board.pieces(P::Pawn) & !board.pieces(P::King);
        if board.colors(loser).len() == 1 && !pieces.is_empty() {
            let (theirs, ours) = (board.king(loser), board.king(winner));
            let edge = |at: i32| (3 - at).max(at - 4);
            let pushed = edge(theirs.file() as i32) + edge(theirs.rank() as i32);
            let apart = (theirs.file() as i32 - ours.file() as i32).abs()
                + (theirs.rank() as i32 - ours.rank() as i32).abs();
            return sign * (10 * pushed + 4 * (14 - apart));
        }
    }
    0
}

/// Where each piece stands best, pawn to queen, from white's side with the
/// eighth rank first.
#[rustfmt::skip]
const SQUARES: [[i32; 64]; 5] = [
    [
          0,   0,   0,   0,   0,   0,   0,   0,
         50,  50,  50,  50,  50,  50,  50,  50,
         10,  10,  20,  30,  30,  20,  10,  10,
          5,   5,  10,  25,  25,  10,   5,   5,
          0,   0,   0,  20,  20,   0,   0,   0,
          5,  -5, -10,   0,   0, -10,  -5,   5,
          5,  10,  10, -20, -20,  10,  10,   5,
          0,   0,   0,   0,   0,   0,   0,   0,
    ],
    [
        -50, -40, -30, -30, -30, -30, -40, -50,
        -40, -20,   0,   0,   0,   0, -20, -40,
        -30,   0,  10,  15,  15,  10,   0, -30,
        -30,   5,  15,  20,  20,  15,   5, -30,
        -30,   0,  15,  20,  20,  15,   0, -30,
        -30,   5,  10,  15,  15,  10,   5, -30,
        -40, -20,   0,   5,   5,   0, -20, -40,
        -50, -40, -30, -30, -30, -30, -40, -50,
    ],
    [
        -20, -10, -10, -10, -10, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,  10,  10,   5,   0, -10,
        -10,   5,   5,  10,  10,   5,   5, -10,
        -10,   0,  10,  10,  10,  10,   0, -10,
        -10,  10,  10,  10,  10,  10,  10, -10,
        -10,   5,   0,   0,   0,   0,   5, -10,
        -20, -10, -10, -10, -10, -10, -10, -20,
    ],
    [
          0,   0,   0,   0,   0,   0,   0,   0,
          5,  10,  10,  10,  10,  10,  10,   5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
         -5,   0,   0,   0,   0,   0,   0,  -5,
          0,   0,   0,   5,   5,   0,   0,   0,
    ],
    [
        -20, -10, -10,  -5,  -5, -10, -10, -20,
        -10,   0,   0,   0,   0,   0,   0, -10,
        -10,   0,   5,   5,   5,   5,   0, -10,
         -5,   0,   5,   5,   5,   5,   0,  -5,
          0,   0,   5,   5,   5,   5,   0,  -5,
        -10,   5,   5,   5,   5,   5,   0, -10,
        -10,   0,   5,   0,   0,   0,   0, -10,
        -20, -10, -10,  -5,  -5, -10, -10, -20,
    ],
];

/// The king with pieces about: tucked away behind its pawns.
#[rustfmt::skip]
const KING_MIDDLE: [i32; 64] = [
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -30, -40, -40, -50, -50, -40, -40, -30,
    -20, -30, -30, -40, -40, -30, -30, -20,
    -10, -20, -20, -20, -20, -20, -20, -10,
     20,  20,   0,   0,   0,   0,  20,  20,
     20,  30,  10,   0,   0,  10,  30,  20,
];

/// The king in the ending: out in the middle, where it can help.
#[rustfmt::skip]
const KING_END: [i32; 64] = [
    -50, -40, -30, -20, -20, -30, -40, -50,
    -30, -20, -10,   0,   0, -10, -20, -30,
    -30, -10,  20,  30,  30,  20, -10, -30,
    -30, -10,  30,  40,  40,  30, -10, -30,
    -30, -10,  30,  40,  40,  30, -10, -30,
    -30, -10,  20,  30,  30,  20, -10, -30,
    -30, -30,   0,   0,   0,   0, -30, -30,
    -50, -30, -30, -30, -30, -30, -30, -50,
];

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use game_api::RoomRng;
    use rand::SeedableRng;

    use super::*;
    use crate::control::TimeControl;
    use crate::game::Players;
    use crate::outcome::Termination;
    use crate::piece::Color;
    use crate::position::START_FEN;

    const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

    fn fen(text: &str) -> Position {
        Position::from_fen(text).unwrap()
    }

    fn uci(text: &str) -> Move {
        Move::parse_uci(text).unwrap()
    }

    fn rng(seed: u64) -> RoomRng {
        RoomRng::seed_from_u64(seed)
    }

    /// Whether the game is over by the board alone: mate, stalemate, the
    /// fifty-move rule or too little left to mate.
    fn over(position: &Position) -> bool {
        !position.has_legal_moves()
            || position.insufficient_material()
            || position.halfmove_clock() >= 100
    }

    /// Plays from the start, white at `levels.0` and black at `levels.1`,
    /// until the game's over or 200 plies; checks every move is legal.
    fn play(levels: (u8, u8), rng: &mut RoomRng) -> Position {
        let mut position = Position::start();
        for _ in 0..200 {
            if over(&position) {
                break;
            }
            let level = match position.side_to_move() {
                Color::White => levels.0,
                Color::Black => levels.1,
            };
            let mv = choose(&position, level, rng).expect("a move while there are legal ones");
            assert!(
                position.is_legal(mv),
                "level {level} played {mv} in {}",
                position.fen()
            );
            position.play(mv).unwrap();
        }
        position
    }

    #[test]
    fn mates_in_one() {
        for (text, mate) in [
            ("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", "a1a8"),
            (
                "rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq g3 0 2",
                "d8h4",
            ),
            ("k7/8/1K6/8/8/8/7Q/8 w - - 0 1", "h2h8"),
            // By castling, which comes back as the king's two-square step.
            // Rd1 is check too, but the king gets out by c2.
            ("8/8/8/8/2ppp3/2pkp3/8/R3K1N1 w Q - 0 1", "e1c1"),
        ] {
            let position = fen(text);
            for level in 2..=5 {
                for seed in 0..4 {
                    let mv = choose(&position, level, &mut rng(seed));
                    assert_eq!(mv, Some(uci(mate)), "level {level}, seed {seed}: {text}");
                }
            }
        }
    }

    #[test]
    fn takes_a_hanging_queen() {
        for (text, take) in [
            ("4k3/8/8/3q4/8/8/3R4/4K3 w - - 0 1", "d2d5"),
            (
                "rnbqkb1r/pppp1ppp/5n2/4p2Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 3",
                "f6h5",
            ),
        ] {
            let position = fen(text);
            for level in 3..=5 {
                for seed in 0..4 {
                    let mv = choose(&position, level, &mut rng(seed));
                    assert_eq!(mv, Some(uci(take)), "level {level}, seed {seed}: {text}");
                }
            }
        }
    }

    #[test]
    fn no_move_once_the_game_is_over() {
        let mated = fen("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3");
        let stalemated = fen("k7/2Q5/1K6/8/8/8/8/8 b - - 0 1");
        for level in LEVELS {
            assert_eq!(choose(&mated, level, &mut rng(0)), None);
            assert_eq!(choose(&stalemated, level, &mut rng(0)), None);
        }
    }

    #[test]
    fn self_play_makes_only_legal_moves() {
        for level in LEVELS {
            let end = play((level, level), &mut rng(level.into()));
            if !end.has_legal_moves() {
                assert_eq!(choose(&end, level, &mut rng(0)), None);
            }
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_move() {
        for text in [START_FEN, KIWIPETE] {
            let position = fen(text);
            for level in LEVELS {
                for seed in 0..8 {
                    let first = choose(&position, level, &mut rng(seed));
                    let again = choose(&position, level, &mut rng(seed));
                    assert_eq!(first, again, "level {level}, seed {seed}: {text}");
                }
            }
        }
        // Level 1 plays differently with different seeds.
        let start = Position::start();
        let moves: Vec<_> = (0..8)
            .map(|seed| choose(&start, 1, &mut rng(seed)))
            .collect();
        assert!(moves.iter().any(|mv| *mv != moves[0]), "{moves:?}");
    }

    #[test]
    fn levels_out_of_range_are_clamped() {
        let position = fen(KIWIPETE);
        assert_eq!(
            choose(&position, 0, &mut rng(3)),
            choose(&position, 1, &mut rng(3))
        );
        assert_eq!(
            choose(&position, 9, &mut rng(3)),
            choose(&position, 5, &mut rng(3))
        );
    }

    #[test]
    fn level_5_beats_level_1() {
        let mut rng = rng(7);
        for (levels, loser) in [((5, 1), Color::Black), ((1, 5), Color::White)] {
            let end = play(levels, &mut rng);
            assert!(
                end.in_check() && !end.has_legal_moves() && end.side_to_move() == loser,
                "{levels:?} ended at {}",
                end.fen()
            );
        }
    }

    #[test]
    fn steers_clear_of_a_third_repetition_when_ahead() {
        let position = fen("8/8/8/4k3/8/8/8/R3K3 w - - 10 40");
        for level in 3..=5 {
            let best = choose(&position, level, &mut rng(1)).unwrap();
            let next = position.after(best).unwrap().repetition_key();
            // Once before, it's only the second time.
            let again = choose_after(&position, &[next], level, &mut rng(1));
            assert_eq!(again, Some(best), "level {level}");
            let other = choose_after(&position, &[next, next], level, &mut rng(1)).unwrap();
            assert_ne!(other, best, "level {level}");
            assert!(position.is_legal(other));
        }
    }

    #[test]
    fn makes_for_a_third_repetition_when_behind() {
        let position = fen("8/8/8/4k3/8/8/Q7/4K3 b - - 10 40");
        for level in 3..=5 {
            let best = choose(&position, level, &mut rng(1)).unwrap();
            let draw = position
                .legal_moves()
                .into_iter()
                .find(|&mv| mv != best)
                .unwrap();
            let next = position.after(draw).unwrap().repetition_key();
            let again = choose_after(&position, &[next], level, &mut rng(1));
            assert_eq!(again, Some(best), "level {level}");
            let taken = choose_after(&position, &[next, next], level, &mut rng(1));
            assert_eq!(taken, Some(draw), "level {level}");
        }
    }

    #[test]
    fn knows_the_games_positions() {
        // The queen and the king step out and back, then out again and the
        // queen back, so black's e5e4 brings the start round a third time.
        let start = fen("8/8/8/8/4k3/8/Q7/4K3 w - - 10 40");
        let players = Players { white: 1, black: 2 };
        let mut game = Game::from_position(start, TimeControl::minutes(1, 0), players);
        let cycle = ["a2a3", "e4e5", "a3a2", "e5e4"];
        for text in cycle.iter().chain(&cycle[..3]) {
            let by = game.side_to_move();
            game.play(by, uci(text), Duration::ZERO).unwrap();
        }
        let draw = uci("e5e4");
        assert_ne!(choose(game.position(), 5, &mut rng(0)), Some(draw));
        assert_eq!(choose_in(&game, 5, &mut rng(0)), Some(draw));
        game.play(Color::Black, draw, Duration::ZERO).unwrap();
        let outcome = game.outcome().unwrap();
        assert_eq!(outcome.termination, Termination::Threefold);
    }

    #[test]
    #[ignore = "times the search; run it in a release build"]
    fn level_5_takes_under_20_ms_a_move() {
        let mut slowest = (Duration::ZERO, String::new());
        for text in [
            START_FEN,
            KIWIPETE,
            "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP1B1PPP/R2QKB1R w KQ - 0 8",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "8/8/8/4k3/8/8/8/4K2R w - - 0 1",
        ] {
            let mut position = fen(text);
            for _ in 0..40 {
                if over(&position) {
                    break;
                }
                // The same move each time, so the quickest of three is how
                // long it takes with nothing else wanting the processor.
                let took = (0..3)
                    .map(|_| {
                        let started = Instant::now();
                        choose(&position, 5, &mut rng(0));
                        started.elapsed()
                    })
                    .min()
                    .unwrap();
                if took > slowest.0 {
                    slowest = (took, position.fen());
                }
                position
                    .play(choose(&position, 5, &mut rng(0)).unwrap())
                    .unwrap();
            }
        }
        assert!(slowest.0 < Duration::from_millis(20), "{slowest:?}");
    }
}
