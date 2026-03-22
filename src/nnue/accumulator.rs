use crate::nnue::get_numa_parameters;
impl Default for AccumulatorCache {
    fn default() -> Self {
        let parameters = get_numa_parameters();
        Self {
            entries: Box::new(std::array::from_fn(|_i| {
                std::array::from_fn(|_j| std::array::from_fn(|_k| CacheEntry::new(parameters)))
            })),
        }
    }
}
use super::{Aligned, L1_SIZE, Parameters, simd};
use crate::{
    nnue::INPUT_BUCKETS,
    types::{Bitboard, Color, PieceType},
};

pub mod psq;
pub mod threats;

pub use psq::PstAccumulator;
pub use threats::ThreatAccumulator;

#[derive(Clone)]
pub struct AccumulatorCache {
    entries: Box<[[[CacheEntry; INPUT_BUCKETS]; 2]; 2]>,
}

#[derive(Clone)]
pub struct CacheEntry {
    values: Aligned<[i16; L1_SIZE]>,
    pieces: [Bitboard; PieceType::NUM],
    colors: [Bitboard; Color::NUM],
}

impl CacheEntry {
    pub fn new(parameters: &Parameters) -> Self {
        Self {
            values: parameters.ft_biases.clone(),
            pieces: [Bitboard::default(); PieceType::NUM],
            colors: [Bitboard::default(); Color::NUM],
        }
    }
}
