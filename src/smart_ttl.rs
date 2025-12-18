use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// Configuration for intelligent TTL management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTtlConfig {
    /// Default TTL for queries that don't match any pattern
    pub default_ttl: Duration,
    
    /// TTL for user profile and authentication data (changes moderately)
    pub user_profile_ttl: Duration,
    
    /// TTL for static content like categories, tags, settings (rarely changes)
    pub static_content_ttl: Duration,
    
    /// TTL for real-time data like live scores, stock prices (changes frequently)
    pub real_time_data_ttl: Duration,
    
    /// TTL for aggregated/calculated data (expensive to compute)
    pub aggregated_data_ttl: Duration,
    
    /// TTL for list/paginated queries (can be stale)
    pub list_query_ttl: Duration,
    
    /// TTL for single item queries (fresher data preferred)
    pub item_query_ttl: Duration,
    
    /// Enable automatic volatility detection (learns from mutation patterns)
    pub auto_detect_volatility: bool,
    
    /// Minimum number of observations before adjusting TTL
    pub min_observations: usize,
    
    /// Maximum TTL adjustment factor (e.g., 2.0 = can double or halve TTL)
    pub max_adjustment_factor: f64,
    
    /// Custom query pattern to TTL mappings
    pub custom_patterns: HashMap<String, Duration>,
    
    /// Enable TTL hints from GraphQL directives (@cacheControl)
    pub respect_cache_hints: bool,
}

impl Default for SmartTtlConfig {
    fn default() -> Self {
        Self {
            default_ttl: Duration::from_secs(300), // 5 minutes
            user_profile_ttl: Duration::from_secs(900), // 15 minutes
            static_content_ttl: Duration::from_secs(86400), // 24 hours
            real_time_data_ttl: Duration::from_secs(5), // 5 seconds
            aggregated_data_ttl: Duration::from_secs(1800), // 30 minutes
            list_query_ttl: Duration::from_secs(600), // 10 minutes
            item_query_ttl: Duration::from_secs(300), // 5 minutes
            auto_detect_volatility: true,
            min_observations: 10,
            max_adjustment_factor: 2.0,
            custom_patterns: HashMap::new(),
            respect_cache_hints: true,
        }
    }
}

/// Tracks query patterns and their volatility for intelligent TTL adjustment
#[derive(Debug)]
pub struct SmartTtlManager {
    config: SmartTtlConfig,
    query_stats: Arc<RwLock<HashMap<String, QueryVolatilityStats>>>,
    mutation_tracker: Arc<RwLock<MutationTracker>>,
}

#[derive(Debug, Clone)]
struct QueryVolatilityStats {
    /// Number of times this query pattern was executed
    hit_count: u64,
    
    /// Number of times the result changed
    change_count: u64,
    
    /// Last result hash (for change detection)
    last_result_hash: u64,
    
    /// Computed volatility score (0.0 = never changes, 1.0 = always changes)
    volatility_score: f64,
    
    /// Recommended TTL based on volatility
    recommended_ttl: Duration,
    
    /// Last update time
    last_update: Instant,
}

#[derive(Debug, Clone, Default)]
struct MutationTracker {
    /// Tracks which queries are affected by which mutations
    mutation_to_queries: HashMap<String, Vec<String>>,
    
    /// Tracks mutation frequency per type
    mutation_frequency: HashMap<String, MutationStats>,
}

#[derive(Debug, Clone)]
struct MutationStats {
    count: u64,
    last_mutation: Instant,
    avg_interval: Duration,
}

impl SmartTtlManager {
    /// Create a new smart TTL manager with the given configuration
    pub fn new(config: SmartTtlConfig) -> Self {
        Self {
            config,
            query_stats: Arc::new(RwLock::new(HashMap::new())),
            mutation_tracker: Arc::new(RwLock::new(MutationTracker::default())),
        }
    }

    /// Calculate the optimal TTL for a given query
    pub async fn calculate_ttl(
        &self,
        query: &str,
        query_type: &str,
        cache_hint: Option<Duration>,
    ) -> TtlResult {
        let start = Instant::now();
        
        // 1. Check for cache control hints from schema
        if self.config.respect_cache_hints && cache_hint.is_some() {
            return TtlResult {
                ttl: cache_hint.unwrap(),
                strategy: TtlStrategy::CacheHint,
                confidence: 1.0,
                calculation_time: start.elapsed(),
            };
        }
        
        // 2. Check custom pattern matches
        for (pattern, ttl) in &self.config.custom_patterns {
            if query.contains(pattern) {
                return TtlResult {
                    ttl: *ttl,
                    strategy: TtlStrategy::CustomPattern(pattern.clone()),
                    confidence: 1.0,
                    calculation_time: start.elapsed(),
                };
            }
        }
        
        // 3. Detect query type and apply appropriate TTL
        let base_ttl = self.detect_query_type_ttl(query, query_type);
        
        // 4. Apply volatility-based adjustment if enabled
        if self.config.auto_detect_volatility {
            if let Some(adjusted_ttl) = self.get_volatility_adjusted_ttl(query).await {
                return TtlResult {
                    ttl: adjusted_ttl.ttl,
                    strategy: TtlStrategy::VolatilityBased {
                        base_ttl,
                        volatility_score: adjusted_ttl.volatility_score,
                    },
                    confidence: adjusted_ttl.confidence,
                    calculation_time: start.elapsed(),
                };
            }
        }
        
        TtlResult {
            ttl: base_ttl,
            strategy: TtlStrategy::QueryType(query_type.to_string()),
            confidence: 0.8,
            calculation_time: start.elapsed(),
        }
    }

    /// Detect query type and return appropriate TTL
    fn detect_query_type_ttl(&self, query: &str, query_type: &str) -> Duration {
        let query_lower = query.to_lowercase();
        
        // Real-time data patterns
        if query_lower.contains("live")
            || query_lower.contains("current")
            || query_lower.contains("realtime")
            || query_lower.contains("now")
        {
            return self.config.real_time_data_ttl;
        }
        
        // Static content patterns
        if query_lower.contains("categories")
            || query_lower.contains("tags")
            || query_lower.contains("settings")
            || query_lower.contains("config")
            || query_lower.contains("metadata")
        {
            return self.config.static_content_ttl;
        }
        
        // User profile patterns
        if query_lower.contains("profile")
            || query_lower.contains("user")
            || query_lower.contains("account")
            || query_lower.contains("me")
        {
            return self.config.user_profile_ttl;
        }
        
        // Aggregated data patterns
        if query_lower.contains("count")
            || query_lower.contains("sum")
            || query_lower.contains("aggregate")
            || query_lower.contains("statistics")
            || query_lower.contains("analytics")
        {
            return self.config.aggregated_data_ttl;
        }
        
        // List queries (plural forms, pagination)
        if query_lower.contains("list")
            || query_lower.contains("page")
            || query_lower.contains("offset")
            || query_lower.contains("limit")
            || query_type.ends_with('s') // plural
        {
            return self.config.list_query_ttl;
        }
        
        // Single item queries
        if query_lower.contains("byid")
            || query_lower.contains("get")
            || query_lower.contains("find")
        {
            return self.config.item_query_ttl;
        }
        
        self.config.default_ttl
    }

    /// Get volatility-adjusted TTL based on historical data
    async fn get_volatility_adjusted_ttl(&self, query: &str) -> Option<VolatilityAdjustedTtl> {
        let stats = self.query_stats.read().await;
        let query_pattern = self.extract_query_pattern(query);
        
        if let Some(volatility_stats) = stats.get(&query_pattern) {
            if volatility_stats.hit_count >= self.config.min_observations as u64 {
                return Some(VolatilityAdjustedTtl {
                    ttl: volatility_stats.recommended_ttl,
                    volatility_score: volatility_stats.volatility_score,
                    confidence: self.calculate_confidence(volatility_stats.hit_count),
                });
            }
        }
        
        None
    }

    /// Record a query execution and result for volatility tracking
    pub async fn record_query_result(&self, query: &str, result_hash: u64) {
        if !self.config.auto_detect_volatility {
            return;
        }
        
        let query_pattern = self.extract_query_pattern(query);
        let mut stats = self.query_stats.write().await;
        
        let volatility_stats = stats.entry(query_pattern.clone()).or_insert(QueryVolatilityStats {
            hit_count: 0,
            change_count: 0,
            last_result_hash: result_hash,
            volatility_score: 0.0,
            recommended_ttl: self.config.default_ttl,
            last_update: Instant::now(),
        });
        
        volatility_stats.hit_count += 1;
        
        // Detect if result changed
        if volatility_stats.last_result_hash != result_hash {
            volatility_stats.change_count += 1;
            volatility_stats.last_result_hash = result_hash;
        }
        
        // Recalculate volatility score
        volatility_stats.volatility_score = 
            volatility_stats.change_count as f64 / volatility_stats.hit_count as f64;
        
        // Adjust recommended TTL based on volatility
        volatility_stats.recommended_ttl = self.calculate_recommended_ttl(
            &query_pattern,
            volatility_stats.volatility_score,
        );
        
        volatility_stats.last_update = Instant::now();
    }

    /// Record a mutation to track affected queries
    pub async fn record_mutation(&self, mutation_type: &str, affected_queries: Vec<String>) {
        let mut tracker = self.mutation_tracker.write().await;
        
        // Update mutation frequency
        let mutation_stats = tracker
            .mutation_frequency
            .entry(mutation_type.to_string())
            .or_insert(MutationStats {
                count: 0,
                last_mutation: Instant::now(),
                avg_interval: Duration::from_secs(3600),
            });
        
        mutation_stats.count += 1;
        mutation_stats.last_mutation = Instant::now();
        
        // Track affected queries
        tracker
            .mutation_to_queries
            .entry(mutation_type.to_string())
            .or_insert_with(Vec::new)
            .extend(affected_queries);
    }

    /// Calculate recommended TTL based on volatility score
    fn calculate_recommended_ttl(&self, query_pattern: &str, volatility_score: f64) -> Duration {
        let base_ttl = self.detect_query_type_ttl(query_pattern, "query");
        
        // High volatility = shorter TTL
        // Low volatility = longer TTL
        let adjustment_factor = if volatility_score > 0.7 {
            // Very volatile (changes 70%+ of the time)
            0.5 // Halve the TTL
        } else if volatility_score > 0.3 {
            // Moderately volatile
            0.75
        } else if volatility_score < 0.1 {
            // Very stable (changes < 10% of the time)
            self.config.max_adjustment_factor // Double the TTL
        } else {
            // Stable
            1.5
        };
        
        let adjusted_secs = (base_ttl.as_secs() as f64 * adjustment_factor) as u64;
        Duration::from_secs(adjusted_secs.max(1)) // Minimum 1 second
    }

    /// Extract query pattern (remove variables, IDs, etc.)
    fn extract_query_pattern(&self, query: &str) -> String {
        // Simplified pattern extraction - remove specific IDs and values
        query
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                // Remove arguments in parentheses
                if let Some(pos) = line.find('(') {
                    &line[..pos]
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Calculate confidence score based on number of observations
    fn calculate_confidence(&self, observations: u64) -> f64 {
        let min = self.config.min_observations as f64;
        let confidence = (observations as f64 - min) / (min * 10.0);
        confidence.min(1.0).max(0.5)
    }

    /// Get analytics about TTL effectiveness
    pub async fn get_analytics(&self) -> SmartTtlAnalytics {
        let stats = self.query_stats.read().await;
        
        if stats.is_empty() {
            return SmartTtlAnalytics::default();
        }
        
        let total_queries = stats.len();
        let avg_volatility: f64 = stats.values()
            .map(|s| s.volatility_score)
            .sum::<f64>() / total_queries as f64;
        
        let avg_ttl_secs: u64 = stats.values()
            .map(|s| s.recommended_ttl.as_secs())
            .sum::<u64>() / total_queries as u64;
        
        let highly_volatile = stats.values()
            .filter(|s| s.volatility_score > 0.7)
            .count();
        
        let stable_queries = stats.values()
            .filter(|s| s.volatility_score < 0.1)
            .count();
        
        SmartTtlAnalytics {
            total_queries,
            avg_volatility_score: avg_volatility,
            avg_recommended_ttl: Duration::from_secs(avg_ttl_secs),
            highly_volatile_queries: highly_volatile,
            stable_queries,
        }
    }

    /// Clean up old statistics (call periodically)
    pub async fn cleanup_old_stats(&self, max_age: Duration) {
        let mut stats = self.query_stats.write().await;
        let now = Instant::now();
        
        stats.retain(|_, stat| {
            now.duration_since(stat.last_update) <= max_age
        });
    }
}

#[derive(Debug, Clone)]
pub struct TtlResult {
    pub ttl: Duration,
    pub strategy: TtlStrategy,
    pub confidence: f64,
    pub calculation_time: Duration,
}

#[derive(Debug, Clone)]
pub enum TtlStrategy {
    /// TTL from @cacheControl directive
    CacheHint,
    
    /// TTL from custom pattern matching
    CustomPattern(String),
    
    /// TTL based on detected query type
    QueryType(String),
    
    /// TTL adjusted based on historical volatility
    VolatilityBased {
        base_ttl: Duration,
        volatility_score: f64,
    },
}

#[derive(Debug, Clone)]
struct VolatilityAdjustedTtl {
    ttl: Duration,
    volatility_score: f64,
    confidence: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SmartTtlAnalytics {
    pub total_queries: usize,
    pub avg_volatility_score: f64,
    pub avg_recommended_ttl: Duration,
    pub highly_volatile_queries: usize,
    pub stable_queries: usize,
}

/// Parse cache control hint from GraphQL @cacheControl directive
pub fn parse_cache_hint(schema_metadata: &str) -> Option<Duration> {
    // Example: @cacheControl(maxAge: 300)
    if let Some(start) = schema_metadata.find("maxAge:") {
        let remaining = &schema_metadata[start + 7..];
        if let Some(end) = remaining.find(|c: char| !c.is_numeric()) {
            if let Ok(seconds) = remaining[..end].trim().parse::<u64>() {
                return Some(Duration::from_secs(seconds));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_static_content_detection() {
        let config = SmartTtlConfig::default();
        let manager = SmartTtlManager::new(config.clone());
        
        let query = "query { categories { id name } }";
        let result = manager.calculate_ttl(query, "categories", None).await;
        
        assert_eq!(result.ttl, config.static_content_ttl);
    }

    #[tokio::test]
    async fn test_real_time_data_detection() {
        let config = SmartTtlConfig::default();
        let manager = SmartTtlManager::new(config.clone());
        
        let query = "query { liveScores { team score } }";
        let result = manager.calculate_ttl(query, "liveScores", None).await;
        
        assert_eq!(result.ttl, config.real_time_data_ttl);
    }

    #[tokio::test]
    async fn test_volatility_tracking() {
        let manager = SmartTtlManager::new(SmartTtlConfig::default());
        
        let query = "query { user(id: 1) { name } }";
        
        // Record same result 10 times (stable data)
        for _ in 0..10 {
            manager.record_query_result(query, 12345).await;
        }
        
        let analytics = manager.get_analytics().await;
        assert_eq!(analytics.total_queries, 1);
        assert!(analytics.avg_volatility_score < 0.1);
    }

    #[tokio::test]
    async fn test_cache_hint_priority() {
        let config = SmartTtlConfig::default();
        let manager = SmartTtlManager::new(config);
        
        let cache_hint = Some(Duration::from_secs(600));
        let result = manager.calculate_ttl("query { test }", "test", cache_hint).await;
        
        assert_eq!(result.ttl, Duration::from_secs(600));
        assert!(matches!(result.strategy, TtlStrategy::CacheHint));
    }

    #[tokio::test]
    async fn test_custom_patterns() {
        let mut config = SmartTtlConfig::default();
        config.custom_patterns.insert(
            "specialQuery".to_string(),
            Duration::from_secs(7200),
        );
        
        let manager = SmartTtlManager::new(config);
        let query = "query { specialQuery { data } }";
        let result = manager.calculate_ttl(query, "specialQuery", None).await;
        
        assert_eq!(result.ttl, Duration::from_secs(7200));
    }
}
