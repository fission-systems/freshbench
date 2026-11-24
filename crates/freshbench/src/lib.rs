//! FreshBench: a decompiler benchmark with its own ground truth and metrics.
//!
//! Independent of DecBench (no DecBench code is used); its metric families
//! follow DecBench's design, which the README credits. See README for what is
//! and is not measured.

// Index loops over parallel per-node arrays read better than zipped iterators here.
#![allow(clippy::needless_range_loop)]

pub mod bands;
pub mod callsites;
pub mod cfg;
pub mod csyntax;
pub mod dataset;
pub mod ged;
pub mod headroom;
pub mod manifest;
pub mod rows;
pub mod score;
pub mod taxonomy;
