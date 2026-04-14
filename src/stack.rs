use std::ops::{Index, IndexMut};
use std::ptr::NonNull;

use crate::types::{MAX_PLY, Move, Piece, Score};

pub struct Stack {
    data: [StackEntry; MAX_PLY + 16],
    sentinel: [[i16; 64]; 13],
}

impl Stack {
    pub fn new() -> Box<Self> {
        let mut stack = Box::new(Self::default());
        let sentinel = unsafe { NonNull::new_unchecked(&raw mut stack.sentinel) };
        for entry in &mut stack.data {
            entry.conthist = sentinel;
            entry.contcorrhist = sentinel;
        }
        stack
    }

    pub fn reset_to_sentinel(&mut self, ply: isize) {
        let sentinel = unsafe { NonNull::new_unchecked(&raw mut self.sentinel) };
        self[ply].conthist = sentinel;
        self[ply].contcorrhist = sentinel;
    }
}

impl Default for Stack {
    fn default() -> Self {
        Self {
            data: [StackEntry::default(); MAX_PLY + 16],
            sentinel: [[0; 64]; 13],
        }
    }
}

#[derive(Copy, Clone)]
pub struct StackEntry {
    pub mv: Move,
    pub piece: Piece,
    pub in_check: bool,
    pub eval: i32,
    pub excluded: Move,
    pub tt_move: Move,
    pub tt_pv: bool,
    pub cutoff_count: i32,
    pub move_count: i32,
    pub reduction: i32,
    pub(crate) conthist: NonNull<[[i16; 64]; 13]>,
    pub(crate) contcorrhist: NonNull<[[i16; 64]; 13]>,
}

unsafe impl Send for StackEntry {}

impl StackEntry {
    pub fn conthist(&self) -> &[[i16; 64]; 13] {
        unsafe { self.conthist.as_ref() }
    }

    pub fn conthist_mut(&self) -> &mut [[i16; 64]; 13] {
        unsafe { &mut *self.conthist.as_ptr() }
    }

    pub fn contcorrhist(&self) -> &[[i16; 64]; 13] {
        unsafe { self.contcorrhist.as_ref() }
    }

    pub fn contcorrhist_mut(&self) -> &mut [[i16; 64]; 13] {
        unsafe { &mut *self.contcorrhist.as_ptr() }
    }
}

impl Default for StackEntry {
    fn default() -> Self {
        Self {
            mv: Move::NULL,
            piece: Piece::None,
            in_check: false,
            eval: Score::NONE,
            excluded: Move::NULL,
            tt_move: Move::NULL,
            tt_pv: false,
            cutoff_count: 0,
            move_count: 0,
            reduction: 0,
            conthist: NonNull::dangling(),
            contcorrhist: NonNull::dangling(),
        }
    }
}

impl Index<isize> for Stack {
    type Output = StackEntry;

    fn index(&self, index: isize) -> &Self::Output {
        debug_assert!(index + 8 >= 0 && index < MAX_PLY as isize + 16);
        &self.data[(index + 8) as usize]
    }
}

impl IndexMut<isize> for Stack {
    fn index_mut(&mut self, index: isize) -> &mut Self::Output {
        debug_assert!(index + 8 >= 0 && index < MAX_PLY as isize + 16);
        &mut self.data[(index + 8) as usize]
    }
}
