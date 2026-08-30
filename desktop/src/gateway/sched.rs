// 组内调度:权重顺序、故障切换候选序列与模型熔断健康簿。
//
// 词汇:
//   - priority(顺序优先):按权重降序稳定排列,健康者按序尝试——权重即
//     "调用顺序",最高者恒先,失败顺延;
//   - weighted(加权随机):健康模型间按权重加权随机(不放回抽样),失败者
//     退出后重抽——权重即分流比例;
//   - 熔断:连续失败达阈值后**开断**一个冷却期,期间调度直接跳过(不再让
//     每个请求都在坏模型上白白超时);冷却期满进入半开,放行下一次尝试,
//     成功复位、失败立即重新开断。
//
// 健康簿是跨请求共享的(同一模型的失败证据对组内后续请求生效);调度本身
// 是纯函数(plan),状态只读,便于单测。

use super::ResolvedCandidate;
use std::collections::HashMap;

/// 连续失败多少次后开断。
pub const FAILURE_THRESHOLD: u32 = 3;
/// 开断持续时长(毫秒)。
pub const COOLDOWN_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelHealth {
    pub consecutive_failures: u32,
    /// 最近一次开断的时刻(毫秒);None = 未开断。
    pub opened_at_ms: Option<u64>,
}

impl ModelHealth {
    /// 成功即复位(半开探测成功、正常成功同路径)。
    pub fn record_success(&mut self) {
        *self = ModelHealth::default();
    }

    pub fn record_failure(&mut self, now_ms: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= FAILURE_THRESHOLD {
            self.opened_at_ms = Some(now_ms);
        }
    }

    pub fn state(&self, now_ms: u64) -> HealthState {
        if self.consecutive_failures == 0 {
            return HealthState::Healthy;
        }
        if self.consecutive_failures < FAILURE_THRESHOLD {
            return HealthState::Degraded;
        }
        match self.opened_at_ms {
            Some(at) if now_ms.saturating_sub(at) < COOLDOWN_MS => HealthState::Open,
            _ => HealthState::Probing,
        }
    }

    /// 调度可见性:Open(冷却中)不可用;Probing(冷却期满半开)放行下一次
    /// 尝试——并发的多个探测都会放行,探测代价由请求方自理,不做单探针闸。
    pub fn is_available(&self, now_ms: u64) -> bool {
        !matches!(self.state(now_ms), HealthState::Open)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HealthState {
    Healthy,
    Degraded,
    Open,
    Probing,
}

impl HealthState {
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthState::Healthy => "healthy",
            HealthState::Degraded => "degraded",
            HealthState::Open => "open",
            HealthState::Probing => "probing",
        }
    }
}

/// xorshift64*。state 必须非零(0 会恒返 0)。
pub(crate) fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// 按权重抽取一个下标(不放回抽样的一步)。weights 全零时退化为均匀。
fn weighted_draw(weights: &[u32], rng: &mut u64) -> usize {
    let total: u128 = weights.iter().map(|w| u128::from(*w)).sum();
    if total == 0 {
        let n = weights.len();
        return if n == 0 { 0 } else { (next_u64(rng) as usize) % n };
    }
    let mut ticket = u128::from(next_u64(rng)) % total;
    for (i, w) in weights.iter().enumerate() {
        if ticket < u128::from(*w) {
            return i;
        }
        ticket -= u128::from(*w);
    }
    weights.len() - 1
}

/// 生成尝试顺序。冷却中的候选被剔除;返回候选克隆的有序列表。
/// 健康簿键 = "<group_id>/<model_id>"。
pub fn plan(
    strategy: &str,
    group_id: &str,
    candidates: &[ResolvedCandidate],
    health: &HashMap<String, ModelHealth>,
    now_ms: u64,
    rng: &mut u64,
) -> Vec<ResolvedCandidate> {
    let available: Vec<&ResolvedCandidate> = candidates
        .iter()
        .filter(|c| {
            let h = health.get(&format!("{group_id}/{}", c.id));
            // 无记录 = 全新条目,可用。
            h.map(|h| h.is_available(now_ms)).unwrap_or(true)
        })
        .collect();
    let mut ordered: Vec<ResolvedCandidate> = Vec::with_capacity(available.len());
    if strategy == super::STRATEGY_WEIGHTED {
        // 加权随机 = 不放回抽样:每轮在剩余候选里按权重抽一个,失败者自然
        // 退出后的重抽就是下一轮。
        let mut remaining: Vec<&ResolvedCandidate> = available;
        while !remaining.is_empty() {
            let weights: Vec<u32> = remaining.iter().map(|c| c.weight).collect();
            let pick = weighted_draw(&weights, rng);
            ordered.push(remaining.remove(pick).clone());
        }
    } else {
        // 顺序优先:权重降序稳定排列(权重相同保持组内定义顺序)。
        let mut sorted: Vec<&ResolvedCandidate> = available;
        sorted.sort_by(|a, b| b.weight.cmp(&a.weight));
        ordered.extend(sorted.into_iter().map(|c| (*c).clone()));
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, _group: &str, weight: u32) -> ResolvedCandidate {
        ResolvedCandidate {
            id: id.into(),
            label: id.into(),
            weight,
            provider: "openai".into(),
            base_url: "https://x".into(),
            api_key: "k".into(),
            model: id.into(),
            unavailable: None,
        }
    }

    fn health_key(group: &str, c: &ResolvedCandidate) -> String {
        format!("{group}/{}", c.id)
    }

    #[test]
    fn priority_orders_by_weight_desc_stable() {
        let cands = vec![cand("a", "g", 1), cand("b", "g", 9), cand("c", "g", 5), cand("d", "g", 5)];
        let mut rng = 1u64;
        let plan = plan("priority", "g", &cands, &HashMap::new(), 0, &mut rng);
        let ids: Vec<&str> = plan.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c", "d", "a"], "权重高者先行;同权重保持定义顺序");
    }

    #[test]
    fn priority_skips_open_breakers() {
        let cands = vec![cand("a", "g", 9), cand("b", "g", 5), cand("c", "g", 1)];
        let mut health = HashMap::new();
        let mut h = ModelHealth::default();
        h.record_failure(100);
        h.record_failure(100);
        h.record_failure(100); // 3 次 → Open
        health.insert(health_key("g", &cands[0]), h);
        let mut rng = 1u64;
        let plan = plan("priority", "g", &cands, &health, 100 + COOLDOWN_MS - 1, &mut rng);
        let ids: Vec<&str> = plan.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c"], "冷却中的模型被调度跳过");
    }

    #[test]
    fn breaker_reopens_after_cooldown_and_resets_on_success() {
        let mut h = ModelHealth::default();
        h.record_failure(0);
        h.record_failure(0);
        h.record_failure(0);
        assert_eq!(h.state(1000), HealthState::Open);
        assert!(!h.is_available(1000));
        // 冷却期满 → 半开放行
        assert_eq!(h.state(COOLDOWN_MS), HealthState::Probing);
        assert!(h.is_available(COOLDOWN_MS));
        // 半开尝试又失败 → 立即重新开断
        h.record_failure(COOLDOWN_MS);
        assert_eq!(h.state(COOLDOWN_MS + 1), HealthState::Open);
        // 成功复位
        h.record_success();
        assert_eq!(h.state(COOLDOWN_MS + 2), HealthState::Healthy);
    }

    #[test]
    fn degraded_is_below_threshold() {
        let mut h = ModelHealth::default();
        h.record_failure(0);
        h.record_failure(0);
        assert_eq!(h.state(1), HealthState::Degraded);
        assert!(h.is_available(1), "未达阈值只降级不停用");
    }

    #[test]
    fn weighted_permutation_is_weight_biased_and_complete() {
        let cands = vec![cand("a", "g", 90), cand("b", "g", 10)];
        let mut rng = 42u64;
        let mut first_a = 0;
        let runs = 2000;
        for _ in 0..runs {
            let plan = plan("weighted", "g", &cands, &HashMap::new(), 0, &mut rng);
            assert_eq!(plan.len(), 2, "不放回抽样必须产出全量候选");
            if plan[0].id == "a" {
                first_a += 1;
            }
        }
        let ratio = first_a as f64 / runs as f64;
        // 90/10 分流:首位占比应落在 0.85~0.95(统计断言,种子固定可复现)
        assert!(ratio > 0.85 && ratio < 0.95, "首位命中 a 的比例 {ratio} 应接近 0.9");
    }

    #[test]
    fn weighted_uniform_when_all_zero_weights() {
        let cands = vec![cand("a", "g", 0), cand("b", "g", 0)];
        let mut rng = 7u64;
        let mut counts = HashMap::new();
        for _ in 0..200 {
            let plan = plan("weighted", "g", &cands, &HashMap::new(), 0, &mut rng);
            *counts.entry(plan[0].id.clone()).or_insert(0) += 1;
        }
        let a = *counts.get("a").unwrap();
        assert!(a > 60 && a < 140, "全零权重退化为均匀,首位 a 出现 {a}/200");
    }

    #[test]
    fn weighted_skips_unavailable() {
        let cands = vec![cand("a", "g", 99), cand("b", "g", 1)];
        let mut health = HashMap::new();
        let mut h = ModelHealth::default();
        h.record_failure(0);
        h.record_failure(0);
        h.record_failure(0);
        health.insert(health_key("g", &cands[0]), h);
        let mut rng = 99u64;
        for _ in 0..50 {
            let plan = plan("weighted", "g", &cands, &health, 1, &mut rng);
            assert_eq!(plan.len(), 1);
            assert_eq!(plan[0].id, "b");
        }
    }

    #[test]
    fn xorshift_never_returns_zero() {
        let mut state = 1u64;
        for _ in 0..10_000 {
            let v = next_u64(&mut state);
            assert_ne!(v, 0);
        }
    }
}
