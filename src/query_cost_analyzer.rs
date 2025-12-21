use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Configuration for query cost analysis and enforcement
#[derive(Debug, Clone)]
pub struct QueryCostConfig {
    /// Maximum allowed cost for a single query (reject if exceeded)
    pub max_cost_per_query: u64,

    /// Base cost assigned to each field in the query
    pub base_cost_per_field: u64,

    /// Multipliers for expensive fields (e.g., "user.posts" = 10x)
    pub field_cost_multipliers: HashMap<String, u64>,

    /// Maximum cost budget per user per time window
    pub user_cost_budget: u64,

    /// Time window for user budget (e.g., 1 minute)
    pub budget_window: Duration,

    /// Whether to track and log expensive queries
    pub track_expensive_queries: bool,

    /// Percentile threshold for "expensive" queries (e.g., 95th percentile)
    pub expensive_percentile: f64,

    /// Whether to enable dynamic cost adjustment based on system load
    pub adaptive_costs: bool,

    /// Cost multiplier during high system load (e.g., 2x during peak)
    pub high_load_multiplier: f64,
}

impl Default for QueryCostConfig {
    fn default() -> Self {
        Self {
            max_cost_per_query: 1000,
            base_cost_per_field: 1,
            field_cost_multipliers: HashMap::new(),
            user_cost_budget: 10_000,
            budget_window: Duration::from_secs(60),
            track_expensive_queries: true,
            expensive_percentile: 0.95,
            adaptive_costs: true,
            high_load_multiplier: 2.0,
        }
    }
}

/// Tracks query costs and enforces budgets
pub struct QueryCostAnalyzer {
    config: QueryCostConfig,
    user_budgets: Arc<RwLock<HashMap<String, UserBudget>>>,
    query_costs: Arc<RwLock<Vec<u64>>>, // Historical costs for percentile calculation
    current_load_factor: Arc<RwLock<f64>>,
}

#[derive(Debug, Clone)]
struct UserBudget {
    spent: u64,
    window_start: Instant,
}

impl QueryCostAnalyzer {
    /// Create a new query cost analyzer with the given configuration
    pub fn new(config: QueryCostConfig) -> Self {
        Self {
            config,
            user_budgets: Arc::new(RwLock::new(HashMap::new())),
            query_costs: Arc::new(RwLock::new(Vec::with_capacity(10_000))),
            current_load_factor: Arc::new(RwLock::new(1.0)),
        }
    }

    /// Calculate the cost of a GraphQL query
    pub async fn calculate_query_cost(&self, query: &str) -> Result<QueryCostResult, String> {
        let start = Instant::now();

        // Parse query and count fields
        let field_count = self.count_fields(query);
        let complexity = self.calculate_complexity(query);

        // Calculate base cost
        let mut total_cost = field_count as u64 * self.config.base_cost_per_field;

        // Apply field-specific multipliers
        for (field_pattern, multiplier) in &self.config.field_cost_multipliers {
            if query.contains(field_pattern) {
                total_cost += total_cost * multiplier / 100;
            }
        }

        // Apply adaptive cost multiplier based on system load
        if self.config.adaptive_costs {
            let load_factor = *self.current_load_factor.read().await;
            total_cost = (total_cost as f64 * load_factor) as u64;
        }

        // Check if query exceeds maximum allowed cost
        if total_cost > self.config.max_cost_per_query {
            return Err(format!(
                "Query cost {} exceeds maximum allowed cost {}",
                total_cost, self.config.max_cost_per_query
            ));
        }

        // Track query cost for analytics
        if self.config.track_expensive_queries {
            let mut costs = self.query_costs.write().await;
            costs.push(total_cost);

            // Keep only last 10k queries to prevent memory growth
            if costs.len() > 10_000 {
                costs.drain(0..5_000);
            }
        }

        Ok(QueryCostResult {
            total_cost,
            field_count,
            complexity,
            calculation_time: start.elapsed(),
        })
    }

    /// Check if a user has budget remaining for a query
    pub async fn check_user_budget(&self, user_id: &str, query_cost: u64) -> Result<(), String> {
        let mut budgets = self.user_budgets.write().await;
        let now = Instant::now();

        let budget = budgets.entry(user_id.to_string()).or_insert(UserBudget {
            spent: 0,
            window_start: now,
        });

        // Reset budget if window has expired
        if now.duration_since(budget.window_start) > self.config.budget_window {
            budget.spent = 0;
            budget.window_start = now;
        }

        // Check if adding this query would exceed budget
        if budget.spent + query_cost > self.config.user_cost_budget {
            return Err(format!(
                "User {} exceeded query cost budget ({}/{} in last {:?})",
                user_id, budget.spent, self.config.user_cost_budget, self.config.budget_window
            ));
        }

        // Deduct cost from budget
        budget.spent += query_cost;

        Ok(())
    }

    /// Update system load factor (0.0 = no load, 2.0 = high load)
    pub async fn update_load_factor(&self, cpu_usage: f64, memory_usage: f64) {
        let load = (cpu_usage + memory_usage) / 2.0;
        let factor = if load > 0.8 {
            self.config.high_load_multiplier
        } else if load > 0.6 {
            1.5
        } else {
            1.0
        };

        *self.current_load_factor.write().await = factor;
    }

    /// Get the cost threshold for "expensive" queries (e.g., 95th percentile)
    pub async fn get_expensive_threshold(&self) -> u64 {
        let costs = self.query_costs.read().await;
        if costs.is_empty() {
            return self.config.max_cost_per_query;
        }

        let mut sorted = costs.clone();
        sorted.sort_unstable();

        let index = ((sorted.len() as f64 * self.config.expensive_percentile) as usize)
            .min(sorted.len() - 1);

        sorted[index]
    }

    /// Get analytics about query costs
    pub async fn get_analytics(&self) -> QueryCostAnalytics {
        let costs = self.query_costs.read().await;

        if costs.is_empty() {
            return QueryCostAnalytics::default();
        }

        let mut sorted = costs.clone();
        sorted.sort_unstable();

        let len = sorted.len();
        let sum: u64 = sorted.iter().sum();

        QueryCostAnalytics {
            total_queries: len,
            average_cost: sum / len as u64,
            median_cost: sorted[len / 2],
            p95_cost: sorted[(len as f64 * 0.95) as usize],
            p99_cost: sorted[(len as f64 * 0.99) as usize],
            max_cost: *sorted.last().unwrap(),
            min_cost: *sorted.first().unwrap(),
        }
    }

    /// Simple field counter (improved version would use GraphQL parser)
    fn count_fields(&self, query: &str) -> usize {
        // Count occurrences of newlines and nested fields
        // This is a simplified heuristic; real implementation should use graphql-parser
        query
            .lines()
            .map(|l| l.trim())
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter(|line| {
                !line.starts_with('}')
                    && !line.starts_with("query")
                    && !line.starts_with("mutation")
                    && !line.starts_with("subscription")
            })
            .filter(|line| *line != "{")
            .count()
    }

    /// Calculate query complexity (depth * breadth)
    fn calculate_complexity(&self, query: &str) -> usize {
        let depth = query.matches('{').count();
        let breadth = self.count_fields(query);
        depth * breadth
    }

    /// Clean up expired user budgets (call periodically)
    pub async fn cleanup_expired_budgets(&self) {
        let mut budgets = self.user_budgets.write().await;
        let now = Instant::now();

        budgets.retain(|_, budget| {
            now.duration_since(budget.window_start) <= self.config.budget_window * 2
        });
    }
}

#[derive(Debug, Clone)]
pub struct QueryCostResult {
    pub total_cost: u64,
    pub field_count: usize,
    pub complexity: usize,
    pub calculation_time: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct QueryCostAnalytics {
    pub total_queries: usize,
    pub average_cost: u64,
    pub median_cost: u64,
    pub p95_cost: u64,
    pub p99_cost: u64,
    pub max_cost: u64,
    pub min_cost: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_query_cost() {
        let config = QueryCostConfig {
            max_cost_per_query: 100,
            base_cost_per_field: 10,
            ..Default::default()
        };

        let analyzer = QueryCostAnalyzer::new(config);

        let query = r#"
            query {
                user {
                    id
                    name
                    email
                }
            }
        "#;

        let result = analyzer.calculate_query_cost(query).await.unwrap();
        assert!(result.total_cost > 0);
        assert_eq!(result.field_count, 4); // user, id, name, email
    }

    #[tokio::test]
    async fn test_expensive_query_rejection() {
        let config = QueryCostConfig {
            max_cost_per_query: 10,
            base_cost_per_field: 10,
            ..Default::default()
        };

        let analyzer = QueryCostAnalyzer::new(config);

        let query = r#"
            query {
                users {
                    id
                    name
                    posts {
                        id
                        title
                    }
                }
            }
        "#;

        let result = analyzer.calculate_query_cost(query).await;
        // Cost > 10, should fail
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_field_multipliers() {
        let mut multipliers = HashMap::new();
        multipliers.insert("expensiveField".to_string(), 100); // +100% cost
        
        let config = QueryCostConfig {
            base_cost_per_field: 10,
            field_cost_multipliers: multipliers,
            ..Default::default()
        };
        
        let analyzer = QueryCostAnalyzer::new(config);
        
        // Use multi-line format to satisfy count_fields heuristic
        let query_normal = "query {\n  normalField\n}";
        let cost_normal = analyzer.calculate_query_cost(query_normal).await.unwrap().total_cost;
        
        let query_expensive = "query {\n  expensiveField\n}";
        let cost_expensive = analyzer.calculate_query_cost(query_expensive).await.unwrap().total_cost;
        
        // normal = 1 field * 10 = 10
        // expensive = 1 field * 10 = 10, plus 100% multiplier = 20
        assert!(cost_expensive > cost_normal);
        assert_eq!(cost_normal, 10);
        assert_eq!(cost_expensive, 20);
    }

    #[tokio::test]
    async fn test_adaptive_costs() {
        let config = QueryCostConfig {
            base_cost_per_field: 10,
            adaptive_costs: true,
            high_load_multiplier: 2.0,
            ..Default::default()
        };
        
        let analyzer = QueryCostAnalyzer::new(config);
        
        let query = "query {\n  field\n}";
        
        // Low load
        analyzer.update_load_factor(0.1, 0.1).await;
        let cost_low = analyzer.calculate_query_cost(query).await.unwrap().total_cost;
        
        // High load
        analyzer.update_load_factor(0.9, 0.9).await;
        let cost_high = analyzer.calculate_query_cost(query).await.unwrap().total_cost;
        
        assert_eq!(cost_low, 10);
        assert_eq!(cost_high, 20); // 10 * 2.0
    }

    #[tokio::test]
    async fn test_user_budget_enforcement() {
        let config = QueryCostConfig {
            user_cost_budget: 100,
            budget_window: Duration::from_secs(60),
            ..Default::default()
        };

        let analyzer = QueryCostAnalyzer::new(config);

        // First query should succeed
        assert!(analyzer.check_user_budget("user1", 50).await.is_ok());

        // Second query should succeed (total 100)
        assert!(analyzer.check_user_budget("user1", 50).await.is_ok());

        // Third query should fail (would exceed budget)
        assert!(analyzer.check_user_budget("user1", 10).await.is_err());
    }
    
    #[tokio::test]
    async fn test_user_budget_expiration() {
        let config = QueryCostConfig {
            user_cost_budget: 100,
            budget_window: Duration::from_millis(50), // Short window
            ..Default::default()
        };
        
        let analyzer = QueryCostAnalyzer::new(config);
        analyzer.check_user_budget("user1", 100).await.unwrap();
        
        // Immediately full
        assert!(analyzer.check_user_budget("user1", 1).await.is_err());
         
        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(60)).await;
        
        // Should succeed now
        assert!(analyzer.check_user_budget("user1", 1).await.is_ok());
    }

    #[tokio::test]
    async fn test_analytics() {
        let analyzer = QueryCostAnalyzer::new(QueryCostConfig::default());

        // Simulate some queries
        for cost in [10, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            let mut costs = analyzer.query_costs.write().await;
            costs.push(cost);
        }

        let analytics = analyzer.get_analytics().await;
        assert_eq!(analytics.total_queries, 10);
        assert_eq!(analytics.average_cost, 55);
        assert_eq!(analytics.median_cost, 60);
        
        // Threshold check
        let threshold = analyzer.get_expensive_threshold().await;
        // 95th percentile of 10 items. Index 9.
        assert_eq!(threshold, 100);
    }
    
    #[tokio::test]
    async fn test_cleanup_expired_budgets() {
        let config = QueryCostConfig {
            budget_window: Duration::from_millis(10), // Very short
            ..Default::default()
        };
        let analyzer = QueryCostAnalyzer::new(config);
        
        analyzer.check_user_budget("userA", 10).await.unwrap();
        
        // Wait 2x window
        tokio::time::sleep(Duration::from_millis(25)).await;
        
        analyzer.cleanup_expired_budgets().await;
        
        let budgets = analyzer.user_budgets.read().await;
        assert!(budgets.is_empty());
    }
}
