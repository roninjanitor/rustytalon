//! Smart routing for multi-provider LLM support.
//!
//! This module provides intelligent query routing between LLM providers
//! based on query complexity, cost optimization, and provider health.
//!
//! # Architecture
//!
//! - `Analyzer`: Scores query complexity using heuristics
//! - `Strategy`: Defines routing policies (cost, quality, balanced)
//! - `SmartRouter`: Routes queries to the best provider

mod analyzer;
pub mod quality;
mod router;
mod strategy;

pub use analyzer::{ComplexityLevel, ComplexityScore, QueryAnalyzer};
pub use router::{
    ProviderHealth, ProviderHealthReport, RoutedResponse, RoutingDecision, SmartRouter,
};
pub use strategy::{RoutingConfig, RoutingStrategy};
