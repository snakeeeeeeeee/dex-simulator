use std::collections::{HashMap, HashSet, VecDeque};

use ethers::types::Address;

use crate::types::{Asset, PoolIdentifier};

/// Token 图结构，记录 token 之间通过池子建立的连通关系。
#[derive(Debug, Clone, Default)]
pub struct TokenGraph {
    adjacency: HashMap<Address, Vec<TokenEdge>>,
    pools: HashSet<PoolIdentifier>,
    pool_assets: HashMap<PoolIdentifier, Vec<Asset>>,
}

#[derive(Debug, Clone)]
struct TokenEdge {
    token: Address,
    pool: PoolIdentifier,
}

impl TokenGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// 将池子加入图中，自动为所有 token 两两建立连通边。
    pub fn add_pool(&mut self, pool: PoolIdentifier, tokens: Vec<Asset>) {
        if tokens.len() < 2 {
            return;
        }
        self.pool_assets.insert(pool.clone(), tokens.clone());
        for i in 0..tokens.len() {
            for j in (i + 1)..tokens.len() {
                let a = tokens[i].address;
                let b = tokens[j].address;
                self.add_edge(a, b, pool.clone());
                self.add_edge(b, a, pool.clone());
            }
        }
        self.pools.insert(pool);
    }

    fn add_edge(&mut self, from: Address, to: Address, pool: PoolIdentifier) {
        self.adjacency
            .entry(from)
            .or_default()
            .push(TokenEdge { token: to, pool });
    }

    /// 判断某个 token 是否已经与白名单集合连通。
    pub fn is_connected_to_whitelist(&self, start: Address, whitelist: &HashSet<Address>) -> bool {
        if whitelist.contains(&start) {
            return true;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(start);
        queue.push_back(start);

        while let Some(token) = queue.pop_front() {
            if let Some(edges) = self.adjacency.get(&token) {
                for edge in edges {
                    if whitelist.contains(&edge.token) {
                        return true;
                    }
                    if visited.insert(edge.token) {
                        queue.push_back(edge.token);
                    }
                }
            }
        }
        false
    }

    /// 返回当前图中已追踪的池子集合（拷贝）。
    pub fn tracked_pools(&self) -> HashSet<PoolIdentifier> {
        self.pools.clone()
    }

    /// 搜索从起点到目标的路径，限制最大跳数（边数量）。
    pub fn find_paths(
        &self,
        start: Address,
        target: Address,
        max_hops: usize,
    ) -> Vec<Vec<PathStep>> {
        if start == target {
            return Vec::new();
        }
        let mut results = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((start, Vec::<PathStep>::new(), HashSet::from([start])));

        while let Some((current, path, visited)) = queue.pop_front() {
            if path.len() >= max_hops {
                continue;
            }
            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    if visited.contains(&edge.token) {
                        continue;
                    }
                    if let Some(step) = self.build_step(current, edge) {
                        let mut next_path = path.clone();
                        next_path.push(step);
                        if edge.token == target {
                            results.push(next_path);
                        } else {
                            let mut next_visited = visited.clone();
                            next_visited.insert(edge.token);
                            queue.push_back((edge.token, next_path, next_visited));
                        }
                    }
                }
            }
        }

        results
    }

    fn build_step(&self, current: Address, edge: &TokenEdge) -> Option<PathStep> {
        let assets = self.pool_assets.get(&edge.pool)?;
        let input = assets
            .iter()
            .find(|asset| asset.address == current)?
            .clone();
        let output = assets
            .iter()
            .find(|asset| asset.address == edge.token)?
            .clone();
        Some(PathStep {
            pool: edge.pool.clone(),
            input,
            output,
        })
    }
}

/// 表示路径中的单跳。
#[derive(Debug, Clone)]
pub struct PathStep {
    pub pool: PoolIdentifier,
    pub input: Asset,
    pub output: Asset,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes.fill(byte);
        Address::from_slice(&bytes)
    }

    fn asset(byte: u8) -> Asset {
        Asset {
            address: addr(byte),
            symbol: format!("T{}", byte),
            decimals: 18,
        }
    }

    fn pool(id: u8) -> PoolIdentifier {
        PoolIdentifier {
            chain_id: 56,
            dex: "pancake".into(),
            address: addr(id),
            pool_type: crate::types::PoolType::PancakeV2,
        }
    }

    #[test]
    fn test_add_pool_and_connectivity() {
        let mut graph = TokenGraph::new();
        graph.add_pool(pool(1), vec![asset(1), asset(2)]);
        graph.add_pool(pool(2), vec![asset(2), asset(3)]);

        let whitelist: HashSet<_> = [asset(1).address].into_iter().collect();
        assert!(graph.is_connected_to_whitelist(asset(3).address, &whitelist));
        assert!(graph.tracked_pools().contains(&pool(1)));
        assert!(graph.tracked_pools().contains(&pool(2)));
    }

    #[test]
    fn test_find_paths() {
        let mut graph = TokenGraph::new();
        let pools = [pool(1), pool(2)];
        graph.add_pool(pools[0].clone(), vec![asset(1), asset(2)]);
        graph.add_pool(pools[1].clone(), vec![asset(2), asset(3)]);

        let paths = graph.find_paths(asset(1).address, asset(3).address, 3);
        assert_eq!(paths.len(), 1);
        let hop = &paths[0];
        assert_eq!(hop.len(), 2);
        assert_eq!(hop[0].pool, pools[0]);
        assert_eq!(hop[1].pool, pools[1]);
    }
}
