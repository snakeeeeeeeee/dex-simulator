use async_trait::async_trait;

use crate::token_graph::{PathStep, TokenGraph};
use crate::types::{Asset, PathLeg};

/// 路径搜索约束条件。
#[derive(Debug, Clone)]
pub struct PathConstraints {
    pub max_hops: usize,
    pub whitelist: Vec<Asset>,
}

/// 路径搜索器接口，负责根据约束生成候选路径。
#[async_trait]
pub trait PathFinder: Send + Sync {
    async fn best_paths(
        &self,
        from: Asset,
        to: Asset,
        constraints: Option<PathConstraints>,
    ) -> Result<Vec<Vec<PathLeg>>, PathFindingError>;
}

/// 基于 TokenGraph 的路径搜索器。
#[derive(Debug, Clone)]
pub struct GraphPathFinder {
    graph: TokenGraph,
}

impl GraphPathFinder {
    pub fn new(graph: TokenGraph) -> Self {
        Self { graph }
    }
}

#[async_trait]
impl PathFinder for GraphPathFinder {
    async fn best_paths(
        &self,
        from: Asset,
        to: Asset,
        constraints: Option<PathConstraints>,
    ) -> Result<Vec<Vec<PathLeg>>, PathFindingError> {
        let max_hops = constraints.as_ref().map(|c| c.max_hops).unwrap_or(3);
        let steps = self.graph.find_paths(from.address, to.address, max_hops);
        if steps.is_empty() {
            return Err(PathFindingError::NotFound);
        }
        Ok(steps
            .into_iter()
            .map(|path| path.into_iter().map(PathLeg::from).collect())
            .collect())
    }
}

impl From<PathStep> for PathLeg {
    fn from(step: PathStep) -> Self {
        PathLeg {
            pool: step.pool,
            input: step.input,
            output: step.output,
            fee: None,
        }
    }
}

/// 路径搜索相关错误。
#[derive(thiserror::Error, Debug)]
pub enum PathFindingError {
    #[error("未找到有效路径")]
    NotFound,
    #[error("计算失败: {0}")]
    Calculation(String),
}
