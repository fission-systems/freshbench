//! FreshBench: a decompiler benchmark with its own ground truth and metrics.
//!
//! Independent of DecBench (no DecBench code is used); its metric families
//! follow DecBench's design, which the README credits. See README for what is
//! and is not measured.

// Index loops over parallel per-node arrays read better than zipped iterators here.
#![allow(clippy::needless_range_loop)]

pub mod adapters;
pub mod bands;
pub mod boundaries;
pub mod callsites;
pub mod cfg;
pub mod clones;
pub mod corpus;
pub mod csyntax;
pub mod dataset;
pub mod dwarf;
pub mod evidence;
pub mod ged;
pub mod ground_truth;
pub mod headroom;
pub mod joern;
pub mod manifest;
pub mod policy;
pub mod prepare;
pub mod provenance;
pub mod recompile;
pub mod report;
pub mod rows;
pub mod score;
pub mod speed;
pub mod sweep;
pub mod taxonomy;
pub mod types;
