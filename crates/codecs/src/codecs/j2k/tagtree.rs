//! Tag-tree (quad-tree) codec for JPEG 2000 packet headers (ITU-T T.800 B.10.2).
//!
//! A tag tree is a hierarchical description of a 2-D array of non-negative
//! integers where each interior node holds the minimum of its four children.
//! Packet headers use two tag trees per precinct — one for code-block inclusion
//! and one for zero-bit-plane counts (B.10.7.2 / B.10.7.5). Values are coded
//! *incrementally*: repeated queries at rising thresholds emit / consume only
//! the bits needed to refine what the decoder already knows, and per-node state
//! persists across queries (across code-blocks and thresholds within a packet
//! sequence).
//!
//! The encode/decode routines take bit callbacks rather than a concrete
//! bit-stream reader/writer, so this module has no dependency on the (future)
//! packet layer: `packet.rs` will pass closures wired to its FF-stuffing bit
//! I/O. The algorithm mirrors the canonical Taubman / OpenJPEG `tgt_encode` /
//! `tgt_decode` procedures.

use super::error::CodecError;

/// Sentinel meaning "value not yet known" (decoder upper bound before any bit
/// establishes it, and the identity for the encoder's min-propagation).
const UNKNOWN: u32 = u32::MAX;

#[derive(Clone)]
struct Node {
    /// Encoder: the (min-propagated) node value. Decoder: current upper bound,
    /// lowered to the exact value once a `1` bit is read.
    value: u32,
    /// Coding-state lower bound `low` for this node (starts at 0).
    low: u32,
    /// Encoder only: a `1` has already been emitted at this node.
    known: bool,
}

impl Node {
    fn new() -> Self {
        Node {
            value: UNKNOWN,
            low: 0,
            known: false,
        }
    }
}

struct Level {
    w: u32,
    #[allow(dead_code)] // kept for symmetry with `w`; not read after construction
    h: u32,
    nodes: Vec<Node>,
}

impl Level {
    #[inline]
    fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.w + x) as usize
    }
}

/// A tag tree over a `leaves_w × leaves_h` grid of non-negative values.
pub struct TagTree {
    /// `levels[0]` is the leaf grid; each subsequent level halves (ceil) both
    /// dimensions; the last level is `1×1` (the root). Empty when there are no
    /// leaves.
    levels: Vec<Level>,
    leaves_w: u32,
    leaves_h: u32,
}

impl TagTree {
    /// Build a tag tree with the given leaf-grid dimensions. A zero dimension
    /// (no leaves) yields an empty tree that ignores `set`/`encode` and decodes
    /// as "not established".
    pub fn new(leaves_w: u32, leaves_h: u32) -> Self {
        let mut levels = Vec::new();
        if leaves_w != 0 && leaves_h != 0 {
            let (mut w, mut h) = (leaves_w, leaves_h);
            loop {
                let nodes = vec![Node::new(); (w as usize) * (h as usize)];
                levels.push(Level { w, h, nodes });
                if w == 1 && h == 1 {
                    break;
                }
                w = w.div_ceil(2);
                h = h.div_ceil(2);
            }
        }
        TagTree {
            levels,
            leaves_w,
            leaves_h,
        }
    }

    #[inline]
    fn in_range(&self, x: u32, y: u32) -> bool {
        !self.levels.is_empty() && x < self.leaves_w && y < self.leaves_h
    }

    /// Set a leaf value (encoder side). Propagates the running minimum up to the
    /// root so every interior node holds the minimum of its descendant leaves.
    pub fn set(&mut self, x: u32, y: u32, value: u32) {
        if !self.in_range(x, y) {
            return;
        }
        let (mut cx, mut cy) = (x, y);
        for level in &mut self.levels {
            let i = level.idx(cx, cy);
            if value < level.nodes[i].value {
                level.nodes[i].value = value;
            }
            cx /= 2;
            cy /= 2;
        }
    }

    /// Clear coding state (`low` and the emitted-bit flags) while keeping the
    /// leaf/interior values. Lets one tree be re-coded from scratch.
    #[allow(dead_code)] // exercised by tagtree tests; not needed by the single-layer pipeline
    pub fn reset(&mut self) {
        for level in &mut self.levels {
            for node in &mut level.nodes {
                node.low = 0;
                node.known = false;
            }
        }
    }

    /// Path of `(level, x, y)` triples from leaf (`index 0`) up to root.
    fn path(&self, x: u32, y: u32) -> Vec<(usize, u32, u32)> {
        let mut path = Vec::with_capacity(self.levels.len());
        let (mut cx, mut cy) = (x, y);
        for l in 0..self.levels.len() {
            path.push((l, cx, cy));
            cx /= 2;
            cy /= 2;
        }
        path
    }

    /// Encode the information needed to establish whether leaf `(x, y)` has value
    /// `< threshold`, emitting each bit through `out`. Walks root → leaf; per
    /// node it emits `0` while the lower bound is below both the threshold and
    /// the node value, and a single `1` when the value is reached below the
    /// threshold. State persists for later (higher-threshold) queries.
    pub fn encode(&mut self, out: &mut dyn FnMut(u32), x: u32, y: u32, threshold: u32) {
        if !self.in_range(x, y) {
            return;
        }
        let path = self.path(x, y);
        let mut low = 0u32;
        // Root (last in path) down to leaf (first).
        for &(l, nx, ny) in path.iter().rev() {
            let level = &mut self.levels[l];
            let i = level.idx(nx, ny);
            let node = &mut level.nodes[i];

            // Carry the running lower bound down; a child can never sit below
            // its parent's established lower bound.
            if low > node.low {
                node.low = low;
            } else {
                low = node.low;
            }

            while low < threshold {
                if low >= node.value {
                    if !node.known {
                        out(1);
                        node.known = true;
                    }
                    break;
                }
                out(0);
                low += 1;
            }
            node.low = low;
        }
    }

    /// Decode the mirror of [`encode`], pulling bits from `next_bit`. Returns
    /// `Ok(true)` when leaf `(x, y)` is established to have value `< threshold`.
    /// A bit source that runs out surfaces as `Err`, never a panic.
    pub fn decode(
        &mut self,
        next_bit: &mut dyn FnMut() -> Result<u32, CodecError>,
        x: u32,
        y: u32,
        threshold: u32,
    ) -> Result<bool, CodecError> {
        if !self.in_range(x, y) {
            return Ok(false);
        }
        let path = self.path(x, y);
        let mut low = 0u32;
        for &(l, nx, ny) in path.iter().rev() {
            let level = &mut self.levels[l];
            let i = level.idx(nx, ny);
            let node = &mut level.nodes[i];

            if low > node.low {
                node.low = low;
            } else {
                low = node.low;
            }

            while low < threshold && low < node.value {
                if next_bit()? != 0 {
                    node.value = low; // value established at this bound
                } else {
                    low += 1;
                }
            }
            node.low = low;
        }

        // Leaf is path[0]; its value is now known iff below the threshold.
        let (l, lx, ly) = path[0];
        let level = &self.levels[l];
        Ok(level.nodes[level.idx(lx, ly)].value < threshold)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
