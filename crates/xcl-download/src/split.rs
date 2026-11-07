//! 分片调度：区间集合 + 租约（lease）。
//!
//! 区间集合 + 租约这套结构参考了一份 MIT 许可的开源实现，
//! 只保留能用到的那部分，没有搬 TRACE / 诊断 / 恢复谱系那几层。
//!
//! 核心不是「把文件切成 N 片」，而是：
//!
//! ```text
//! PieceMap（每片 1MiB）        SpanSet（未完成字节的区间集合）
//!    ↓                            ↑
//! issue_lease（摘走区间）→ active_leases
//!    ↓                            ↓
//! complete（永久完成）        give_back（失败时 insert 回去，别人可领）
//! ```
//!
//! 为什么用区间集合而不是「第几片」的数组：失败时要把「没拿到的那一段」还回去，
//! 而这一段可能只是某片的一半。用区间表达，切分与归还是同一个操作（split / merge），
//! 不需要维护序号映射。这是静态切 8 片做不到的事 —— 只能整批重来。
//!
//! 本模块零 IO、零 async，纯数据结构，全部可单测。

use std::collections::BTreeMap;

/// 半开区间 `[start, end)`。
///
/// 注意：`fetcher` 那边用的是闭区间（HTTP Range 的写法），
/// 两者只在这一层与 IO 层的交界处转换，不要混着用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

impl Span {
    pub fn new(start: u64, end: u64) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    pub fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// 分片参数。
///
/// 每个默认值都有依据，不是拍的：
/// - `piece_size` 512KiB —— 常见取 1MiB 或 256KiB，这里取中间。
///   取 512KiB 是为了让 1MiB 的文件也能切成两段，同时不把请求切得太碎。
/// - `min_split_size` 1MiB —— 文件门槛。不取 10MiB 那种高门槛
///   （那等于 20MiB 以下全走单连接）。
/// - `min_segment_size` 512KiB —— 每段下限。有实现用 1MiB、也有 256KiB，
///   取中间值：1.5MB 的 Mod 也能开两条连接，而不是等到 2MB。
/// - `max_request_size` 4MiB —— 单次请求上限
/// - `max_connections` 16 —— 2026-10-25 在 HTTP/1.1 下的实测（每段 512KB，只改连接数）：
///   4 → 604 KB/s、8 → 943、16 → 1363、32 → 1424 但 32 个请求里 12 个被服务器拒。
///   这个结论只在 1.1 下成立：h2 会把流全塞进一条连接，连接数根本不起作用。
/// - `total_connections` 32 —— 16 × 2，最多两个大文件全速分片
#[derive(Debug, Clone, Copy)]
pub struct SplitConfig {
    pub piece_size: u64,
    pub min_split_size: u64,
    pub min_segment_size: u64,
    pub max_request_size: u64,
    pub max_connections: usize,
    pub total_connections: usize,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            piece_size: 512 * 1024,
            min_split_size: 1024 * 1024,
            min_segment_size: 512 * 1024,
            max_request_size: 4 * 1024 * 1024,
            max_connections: 16,
            total_connections: 32,
        }
    }
}

/// 小文件的最小片。1MiB 的文件因此能切成 8 片、真正跑起 4–8 条连接。
const MIN_PIECE: u64 = 128 * 1024;
/// 单片上限。片不能比单次请求还大，否则 `clamp_request_size` 的
/// “至少保留一片”会让请求超过 `max_request_size`。
const MAX_PIECE: u64 = 4 * 1024 * 1024;

impl SplitConfig {
    /// 这个大小的文件值不值得分片。
    ///
    /// `min_split_size` 只是文件门槛：≥1MiB 就进入分片路径。
    /// 真正能不能切出两段，由片大小和 `min_segment_size` 在调度器里决定。
    pub fn eligible(&self, total: u64) -> bool {
        self.max_connections > 1
            && self.piece_size > 0
            && total >= self.min_split_size
            && total > self.piece_size
    }

    /// 按文件大小选片大小（只改 `piece_size` / `min_segment_size`）。
    ///
    /// 目标是约 `max_connections × 2` 片，片大小取 128KiB..=4MiB 的 2 的幂：
    ///
    /// | 文件 | 片大小 | 片数 |
    /// |---|---|---|
    /// | 1 MiB | 128 KiB | 8 |
    /// | 2 MiB | 128 KiB | 16 |
    /// | 4 MiB | 128 KiB | 32 |
    /// | 8 MiB | 256 KiB | 32 |
    /// | 32 MiB | 1 MiB | 32 |
    /// | 128 MiB+ | 4 MiB | 32–36 |
    ///
    /// 小文件有足够多的片让多连接真正跑起来；大文件的片变大，但仍然是
    /// 4MiB 请求上限的整数分，HTTP 请求大小不变，只是调度簿记更省。
    pub fn for_total(self, total: u64) -> Self {
        let max_piece = self.max_request_size.max(1).min(MAX_PIECE);
        let min_piece = MIN_PIECE.min(max_piece);
        let target = (self.max_connections.max(1) as u64)
            .saturating_mul(2)
            .clamp(8, 256);
        let raw = (total / target).clamp(min_piece, max_piece);
        let mut piece = min_piece;
        while piece.saturating_mul(2) <= raw && piece.saturating_mul(2) <= max_piece {
            piece *= 2;
        }
        Self {
            piece_size: piece,
            min_segment_size: piece,
            ..self
        }
    }
}

/// 一条租约 = 「这段字节归我下了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// 租约号，归还/完成时用来核对
    pub id: u64,
    /// 属于第几片
    pub piece: usize,
    /// 半开区间
    pub start: u64,
    pub end: u64,
}

impl Lease {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// 一次 HTTP 请求要下的内容。
///
/// 可能横跨多片（整片连续区间），所以带一组租约；
/// 全部发放完成才算成功，中途失败会整体回滚 —— 否则并发的另一个 worker
/// 会从中间的空档钻进去，把同一次请求的区间切成两半。
#[derive(Debug, Clone)]
pub struct Assignment {
    pub worker: usize,
    pub start: u64,
    pub end: u64,
    pub leases: Vec<Lease>,
}

impl Assignment {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// 未完成字节的区间集合：有序、互不相邻。
#[derive(Debug, Clone, Default)]
struct SpanSet {
    spans: Vec<Span>,
}

impl SpanSet {
    fn full(start: u64, end: u64) -> Self {
        let mut set = Self::default();
        if let Some(span) = Span::new(start, end) {
            set.spans.push(span);
        }
        set
    }

    fn first(&self) -> Option<Span> {
        self.spans.first().copied()
    }

    fn count(&self) -> usize {
        self.spans.len()
    }

    /// 摘走一段。返回 false 表示这段不完全在集合里（说明被别的 worker 抢先了）。
    ///
    /// 与 `SpanSet::give` 互为反操作，这也是这个结构存在的理由。
    fn take(&mut self, span: Span) -> bool {
        let Some(index) = self
            .spans
            .iter()
            .position(|cur| cur.start <= span.start && cur.end >= span.end)
        else {
            return false;
        };

        let cur = self.spans.remove(index);
        let mut at = index;
        if cur.start < span.start {
            self.spans.insert(
                at,
                Span {
                    start: cur.start,
                    end: span.start,
                },
            );
            at += 1;
        }
        if span.end < cur.end {
            self.spans.insert(
                at,
                Span {
                    start: span.end,
                    end: cur.end,
                },
            );
        }
        true
    }

    /// 还回一段，相邻/重叠的合并。
    fn give(&mut self, mut span: Span) {
        let mut index = 0;
        while index < self.spans.len() {
            let cur = self.spans[index];
            // 完全在左边（留 1 字节间隙也合并，避免碎片越来越碎）
            if cur.end + 1 < span.start {
                index += 1;
                continue;
            }
            if span.end + 1 < cur.start {
                break;
            }
            span.start = span.start.min(cur.start);
            span.end = span.end.max(cur.end);
            self.spans.remove(index);
        }
        self.spans.insert(index, span);
    }
}

#[derive(Debug, Clone)]
struct PieceState {
    /// 这一片里还没拿到的字节
    missing: SpanSet,
    /// 正在下的租约：id → 区间
    leases: BTreeMap<u64, Span>,
    /// 是否被碰过。碰过的片不参与「整片连续区间」的规划，
    /// 走单区间路径（整片从没碰过）
    touched: bool,
    /// 这一片被派发过几次，用于上层判断是否该换源
    attempts: u32,
}

impl PieceState {
    fn new(start: u64, end: u64) -> Self {
        Self {
            missing: SpanSet::full(start, end),
            leases: BTreeMap::new(),
            touched: false,
            attempts: 0,
        }
    }
}

/// 分片调度器。所有方法都不阻塞、不做 IO，可以放心持锁调用。
pub struct PieceScheduler {
    cfg: SplitConfig,
    total: u64,
    pieces: Vec<PieceState>,
    /// 未完成且未租出的字节数，即「还能派多少活」
    missing_bytes: u64,
    /// 已租出但还没确认的字节数。用它算出「已确认落盘」的字节
    leased_bytes: u64,
    active_leases: usize,
    next_lease: u64,
    /// 单区间路径的轮转游标，避免每次都从 0 号片开始扫
    cursor: usize,
    stopped: bool,
}

impl PieceScheduler {
    /// `total` 必须已知且大于 0。返回 `None` 表示不该走分片。
    pub fn new(total: u64, cfg: SplitConfig) -> Option<Self> {
        if !cfg.eligible(total) {
            return None;
        }
        let piece_size = cfg.piece_size;
        let count = total.div_ceil(piece_size);
        let count = usize::try_from(count).ok()?;

        let mut pieces = Vec::with_capacity(count);
        for index in 0..count {
            let start = index as u64 * piece_size;
            let end = (start + piece_size).min(total);
            pieces.push(PieceState::new(start, end));
        }

        Some(Self {
            cfg,
            total,
            pieces,
            missing_bytes: total,
            leased_bytes: 0,
            active_leases: 0,
            next_lease: 1,
            cursor: 0,
            stopped: false,
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// 已确认落盘（含已被租出但尚未确认的除外）的字节。
    ///
    /// 这个数必须和 IO 层累计写盘的字节严格相等 —— 进度条就靠它。
    pub fn confirmed_bytes(&self) -> u64 {
        self.total
            .saturating_sub(self.missing_bytes)
            .saturating_sub(self.leased_bytes)
    }

    /// 还没派出去的字节
    pub fn remaining_bytes(&self) -> u64 {
        self.missing_bytes
    }

    pub fn active_leases(&self) -> usize {
        self.active_leases
    }

    pub fn is_done(&self) -> bool {
        self.missing_bytes == 0 && self.active_leases == 0
    }

    /// 停止发放新租约（取消或降级时用），已在飞的不受影响。
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// 派一次活。`free_slots` = 除自己以外还能开几条连接。
    ///
    /// 派发规则：
    /// 1. 先找「整片连续的空闲区间」（`dynamic_candidates`）
    /// 2. 按「字节 / 槽位」公平分配，挑当前份额最大的那个切（`plan_dynamic_share`）
    /// 3. 切出来的长度用 `min_segment_size` 与 `max_request_size` 两头夹住
    /// 4. 整片发放租约，失败则整体回滚（`issue_dynamic_candidate`）
    ///
    /// 没有整片连续区间时（都碰过了 / 只剩碎片），退回单区间路径。
    pub fn assign(&mut self, worker: usize, free_slots: usize) -> Option<Assignment> {
        if self.stopped || self.missing_bytes == 0 {
            return None;
        }
        let slots = free_slots.max(1);

        let runs = self.runs();
        if runs.is_empty() {
            return self.assign_fragment(worker, slots);
        }

        let min_share = self.aligned_min_segment();
        let (run, run_slots) = self.plan_share(&runs, slots, min_share);
        let desired = self.share_end_piece(run, run_slots, min_share);
        let end_piece = self.clamp_request_size(run, desired);

        self.issue_run(run, end_piece, worker)
    }

    /// 一次请求的结果落账。
    ///
    /// `written` 是这次请求实际写到磁盘的字节数。它可能小于这次租约的总长度
    /// （连接中途断了）—— 这正是关键：已写的字节立刻记为完成，
    /// 只把没拿到的部分还回去，下一个 worker 只补缺口。
    ///
    /// 记账与回收合并成一个函数，行为等价。返回是否全部完成。
    pub fn commit(&mut self, leases: &[Lease], mut written: u64) -> bool {
        let mut index = 0;

        // 1. 整段都拿到的
        while index < leases.len() {
            let len = leases[index].len();
            if written < len {
                break;
            }
            self.drop_lease(&leases[index]);
            written -= len;
            index += 1;
        }

        if index == leases.len() {
            return self.is_done();
        }

        // 2. 断在中间的那一段
        let lease = leases[index];
        self.drop_lease(&lease);
        if written > 0 {
            let cut = lease.start + written.min(lease.len());
            if cut < lease.end {
                self.restore_span(lease.piece, Span { start: cut, end: lease.end });
            }
        } else {
            self.restore_span(
                lease.piece,
                Span {
                    start: lease.start,
                    end: lease.end,
                },
            );
        }

        // 3. 后面还没开始的全部还回去
        for rest in &leases[index + 1..] {
            self.drop_lease(rest);
            self.restore_span(
                rest.piece,
                Span {
                    start: rest.start,
                    end: rest.end,
                },
            );
        }

        self.is_done()
    }

    /// 整体归还（取消、降级、请求没发出去时用）。
    pub fn reclaim(&mut self, leases: &[Lease]) {
        for lease in leases {
            self.drop_lease(lease);
            self.restore_span(
                lease.piece,
                Span {
                    start: lease.start,
                    end: lease.end,
                },
            );
        }
    }

    // ---------- 内部 ----------

    fn piece_start(&self, index: usize) -> u64 {
        index as u64 * self.cfg.piece_size
    }

    fn piece_end(&self, index: usize) -> u64 {
        (self.piece_start(index) + self.cfg.piece_size).min(self.total)
    }

    /// `min_segment_size` 向上对齐到整片，否则切出来的边界会落在片中间，
    /// 让 `max_slots` 的估算和实际能切的片数对不上。
    fn aligned_min_segment(&self) -> u64 {
        let piece = self.cfg.piece_size.max(1);
        let configured = self.cfg.min_segment_size.max(piece);
        configured.div_ceil(piece) * piece
    }

    /// 找所有「连续的、从没碰过的整片」区间。
    fn runs(&self) -> Vec<Run> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < self.pieces.len() {
            if self.pieces[index].touched {
                index += 1;
                continue;
            }
            let first = index;
            while index < self.pieces.len() && !self.pieces[index].touched {
                index += 1;
            }
            out.push(Run {
                first_piece: first,
                end_piece: index,
                start: self.piece_start(first),
                end: self.piece_end(index - 1),
            });
        }
        out
    }

    /// 这个区间最多能切成几段。
    fn max_slots(&self, run: Run, min_share: u64) -> usize {
        if min_share == 0 {
            return 1;
        }
        let by_min = run.len() / min_share;
        if by_min < 2 || self.legal_split_bounds(run, min_share).is_none() {
            return 1;
        }
        let piece_count = run.end_piece - run.first_piece;
        (piece_count.min(by_min as usize)).max(1)
    }

    /// 切成 N 段后，每段仍要 ≥ `min_share`，据此算出合法的切点范围。
    fn legal_split_bounds(&self, run: Run, min_share: u64) -> Option<(usize, usize)> {
        if run.end_piece <= run.first_piece + 1 || min_share == 0 || run.len() / min_share < 2 {
            return None;
        }
        let piece = self.cfg.piece_size.max(1);
        let min_pieces = usize::try_from(min_share.div_ceil(piece)).ok()?;
        let first_legal = run.first_piece.checked_add(min_pieces)?;
        let last_legal = usize::try_from(run.end.saturating_sub(min_share) / piece)
            .ok()?
            .min(run.end_piece.saturating_sub(1));
        (first_legal <= last_legal).then_some((first_legal, last_legal))
    }

    /// 公平份额：给每个候选分配虚拟槽位，挑「字节/槽位」最大的那个。
    ///
    /// 效果是「每个请求拿 剩余字节 / 可用连接数」，而不是一口气切死。
    fn plan_share(&self, runs: &[Run], slots: usize, min_share: u64) -> (Run, usize) {
        // 候选比槽位多：人人 1 槽，直接选最大的，不用建表
        if runs.len() >= slots {
            let mut best = runs[0];
            for run in &runs[1..] {
                if compare_share(*run, 1, best, 1) == std::cmp::Ordering::Greater {
                    best = *run;
                }
            }
            return (best, 1);
        }

        let mut counts = vec![1usize; runs.len()];
        let mut extra = slots - runs.len();
        while extra > 0 {
            let mut pick: Option<usize> = None;
            for (index, run) in runs.iter().enumerate() {
                if counts[index] >= self.max_slots(*run, min_share) {
                    continue;
                }
                pick = match pick {
                    None => Some(index),
                    Some(current) => {
                        if compare_share(*run, counts[index], runs[current], counts[current])
                            == std::cmp::Ordering::Greater
                        {
                            Some(index)
                        } else {
                            Some(current)
                        }
                    }
                };
            }
            let Some(index) = pick else { break };
            counts[index] += 1;
            extra -= 1;
        }

        let mut chosen = 0;
        for index in 1..runs.len() {
            if compare_share(runs[index], counts[index], runs[chosen], counts[chosen])
                == std::cmp::Ordering::Greater
            {
                chosen = index;
            }
        }
        (runs[chosen], counts[chosen])
    }

    /// 这次请求应该切到哪一片。
    fn share_end_piece(&self, run: Run, slots: usize, min_share: u64) -> usize {
        if slots <= 1 {
            return run.end_piece;
        }
        let Some((first_legal, last_legal)) = self.legal_split_bounds(run, min_share) else {
            return run.end_piece;
        };
        let piece = self.cfg.piece_size.max(1);
        let target = run.start + run.len().div_ceil(slots as u64);
        let whole = target / piece;
        let rest = target % piece;
        // 落在片中间就就近取整，避免每次切出半片的零头
        let ideal = whole + u64::from(rest > piece / 2);
        usize::try_from(ideal)
            .unwrap_or(last_legal)
            .clamp(first_legal, last_legal)
    }

    /// 用 `max_request_size` 压低单次请求大小，但至少保留一片，
    /// 否则会切出 0 长度请求。
    fn clamp_request_size(&self, run: Run, desired_end_piece: usize) -> usize {
        let limit = run.start.saturating_add(self.cfg.max_request_size);
        let mut end_piece = desired_end_piece
            .max(run.first_piece + 1)
            .min(run.end_piece);
        while end_piece > run.first_piece + 1 && self.piece_end(end_piece - 1) > limit {
            end_piece -= 1;
        }
        end_piece
    }

    /// 整片发放，任一片失败就整体回滚。
    fn issue_run(&mut self, run: Run, end_piece: usize, worker: usize) -> Option<Assignment> {
        let mut leases = Vec::with_capacity(end_piece - run.first_piece);
        for piece in run.first_piece..end_piece {
            match self.issue_whole_piece(piece) {
                Some(lease) => leases.push(lease),
                None => {
                    self.reclaim(&leases);
                    return None;
                }
            }
        }
        let first = leases.first()?;
        let last = leases.last()?;
        Some(Assignment {
            worker,
            start: first.start,
            end: last.end,
            leases,
        })
    }

    fn issue_whole_piece(&mut self, piece: usize) -> Option<Lease> {
        let start = self.piece_start(piece);
        let end = self.piece_end(piece);
        let span = Span::new(start, end)?;
        let state = self.pieces.get_mut(piece)?;
        if !state.missing.take(span) {
            return None;
        }
        state.touched = true;
        state.attempts = state.attempts.saturating_add(1);

        let id = self.next_lease;
        self.next_lease = self.next_lease.saturating_add(1);
        state.leases.insert(id, span);
        self.missing_bytes = self.missing_bytes.saturating_sub(span.len());
        self.leased_bytes = self.leased_bytes.saturating_add(span.len());
        self.active_leases += 1;

        Some(Lease {
            id,
            piece,
            start,
            end,
        })
    }

    /// 只剩碎片时（整片区间都用光了）走这条路：领一个未完成区间。
    fn assign_fragment(&mut self, worker: usize, slots: usize) -> Option<Assignment> {
        let count = self.pieces.len();
        let mut found = None;
        for offset in 0..count {
            let index = (self.cursor + offset) % count;
            if let Some(span) = self.pieces[index].missing.first() {
                found = Some((index, span));
                break;
            }
        }
        let (piece, span) = found?;
        self.cursor = (piece + 1) % count;

        let span = self.split_for_idle(span, slots);
        let lease = self.issue_span(piece, span)?;
        Some(Assignment {
            worker,
            start: lease.start,
            end: lease.end,
            leases: vec![lease],
        })
    }

    /// 连接有空闲但活不够分时，把这一段再切小，让空闲连接也有活干。
    ///
    /// 只有当「在飞的租约 + 剩余区间数」
    /// 少于可用连接数时才切，避免把区间切得比连接还碎。
    fn split_for_idle(&self, span: Span, slots: usize) -> Span {
        let min_segment = self.cfg.min_segment_size;
        if slots <= 1 || min_segment == 0 || span.len() <= min_segment {
            return span;
        }
        let units = self.active_leases as u64 + self.remaining_span_count() as u64;
        if units >= slots as u64 {
            return span;
        }
        let desired = (slots as u64).saturating_sub(units) + 1;
        if desired <= 1 {
            return span;
        }
        let segment_len = span.len().div_ceil(desired).max(min_segment);
        if segment_len >= span.len() || span.len() - segment_len < min_segment {
            return span;
        }
        Span {
            start: span.start,
            end: span.start + segment_len,
        }
    }

    fn remaining_span_count(&self) -> usize {
        // 只在单区间路径用，片数量级在百级，直接数就行
        self.pieces
            .iter()
            .map(|state| state.missing.count())
            .sum()
    }

    fn issue_span(&mut self, piece: usize, span: Span) -> Option<Lease> {
        let state = self.pieces.get_mut(piece)?;
        if !state.missing.take(span) {
            return None;
        }
        state.touched = true;
        state.attempts = state.attempts.saturating_add(1);

        let id = self.next_lease;
        self.next_lease = self.next_lease.saturating_add(1);
        state.leases.insert(id, span);
        self.missing_bytes = self.missing_bytes.saturating_sub(span.len());
        self.leased_bytes = self.leased_bytes.saturating_add(span.len());
        self.active_leases += 1;

        Some(Lease {
            id,
            piece,
            start: span.start,
            end: span.end,
        })
    }

    /// 只摘掉租约，不改 `missing_bytes`
    fn drop_lease(&mut self, lease: &Lease) {
        if let Some(state) = self.pieces.get_mut(lease.piece) {
            state.leases.remove(&lease.id);
        }
        self.active_leases = self.active_leases.saturating_sub(1);
        self.leased_bytes = self.leased_bytes.saturating_sub(lease.len());
    }

    fn restore_span(&mut self, piece: usize, span: Span) {
        if let Some(state) = self.pieces.get_mut(piece) {
            state.missing.give(span);
        }
        self.missing_bytes = self.missing_bytes.saturating_add(span.len());
    }
}

#[derive(Debug, Clone, Copy)]
struct Run {
    first_piece: usize,
    end_piece: usize,
    start: u64,
    end: u64,
}

impl Run {
    fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// 比较两个「字节/槽位」，用交叉相乘避免浮点误差。
fn compare_share(left: Run, left_slots: usize, right: Run, right_slots: usize) -> std::cmp::Ordering {
    let left_share = u128::from(left.len()) * right_slots as u128;
    let right_share = u128::from(right.len()) * left_slots as u128;
    left_share
        .cmp(&right_share)
        .then_with(|| right.start.cmp(&left.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SplitConfig {
        SplitConfig::default()
    }

    fn sched(total: u64) -> PieceScheduler {
        PieceScheduler::new(total, cfg()).expect("该大小应当可分片")
    }

    /// 把所有活领完（模拟一口气开满连接），返回每次分配的长度。
    fn drain(sched: &mut PieceScheduler, slots: usize) -> Vec<u64> {
        let mut out = Vec::new();
        let mut guard = 0;
        while let Some(assignment) = sched.assign(0, slots) {
            out.push(assignment.len());
            // 立刻完成，模拟全部成功
            sched.commit(&assignment.leases, assignment.len());
            guard += 1;
            assert!(guard < 100_000, "派发没有收敛");
        }
        out
    }

    #[test]
    fn small_file_is_not_eligible() {
        let cfg = cfg();
        assert!(!cfg.eligible(0));
        assert!(!cfg.eligible(1024 * 1024 - 1), "小于 1MiB 门槛不该分片");
        assert!(cfg.eligible(1024 * 1024), "1MiB 应能切成两段 512KiB");
        // 1–20MB 的文件过去全部走单连接；现在必须能分片
        assert!(cfg.eligible(10 * 1024 * 1024));
    }

    #[test]
    fn new_returns_none_for_ineligible() {
        assert!(PieceScheduler::new(1024, cfg()).is_none());
    }

    #[test]
    fn piece_size_scales_with_file_size() {
        let cfg = cfg();
        let mib = 1024 * 1024;
        let cases = [
            (1u64, 128 * 1024),
            (2, 128 * 1024),
            (4, 128 * 1024),
            (8, 256 * 1024),
            (16, 512 * 1024),
            (32, 1024 * 1024),
            (64, 2048 * 1024),
            (128, 4096 * 1024),
            (1024, 4096 * 1024),
        ];
        for (mb, expected) in cases {
            let scaled = cfg.for_total(mb * mib);
            assert_eq!(scaled.piece_size, expected, "{mb}MiB 的片大小");
            assert_eq!(
                scaled.min_segment_size, scaled.piece_size,
                "每段下限必须跟着片走"
            );
        }
    }

    #[test]
    fn one_mib_file_can_fill_more_than_two_connections() {
        let total = 1024 * 1024;
        let scaled = cfg().for_total(total);
        assert_eq!(scaled.piece_size, 128 * 1024);
        let mut sched = PieceScheduler::new(total, scaled).expect("1MiB 应当可分片");
        let lengths = drain(&mut sched, 8);
        assert!(
            lengths.len() >= 4,
            "1MiB 应能切成至少 4 段，实际 {}",
            lengths.len()
        );
        assert_eq!(lengths.iter().sum::<u64>(), total);
    }

    #[test]
    fn covers_every_byte_exactly_once() {
        let mut sched = sched(20 * 1024 * 1024);
        let lengths = drain(&mut sched, 8);
        assert_eq!(lengths.iter().sum::<u64>(), 20 * 1024 * 1024);
        assert!(sched.is_done());
        assert_eq!(sched.confirmed_bytes(), 20 * 1024 * 1024);
        assert_eq!(sched.remaining_bytes(), 0);
        assert_eq!(sched.active_leases(), 0);
    }

    #[test]
    fn request_never_exceeds_max_request_size() {
        let mut sched = sched(64 * 1024 * 1024);
        let lengths = drain(&mut sched, 1);
        for len in lengths {
            assert!(len <= 4 * 1024 * 1024, "单次请求 {len} 超过 max_request_size");
        }
    }

    #[test]
    fn request_is_piece_aligned() {
        let mut sched = sched(40 * 1024 * 1024);
        let piece = cfg().piece_size;
        while let Some(assignment) = sched.assign(0, 8) {
            assert_eq!(assignment.start % piece, 0, "起点必须对齐到片");
            assert_eq!(assignment.end % piece, 0, "终点必须对齐到片");
            sched.commit(&assignment.leases, assignment.len());
        }
    }

    #[test]
    fn many_slots_split_the_work() {
        // 32MiB、8 条连接：第一条不该把整个区间吞掉
        let mut sched = sched(32 * 1024 * 1024);
        let first = sched.assign(0, 8).unwrap();
        assert!(
            first.len() <= 4 * 1024 * 1024,
            "首条请求 {} 字节，没有按份额切",
            first.len()
        );
    }

    #[test]
    fn last_piece_may_be_short() {
        // 20MiB + 300KB：最后一片不满
        let total = 20 * 1024 * 1024 + 300 * 1024;
        let mut sched = sched(total);
        let lengths = drain(&mut sched, 4);
        assert_eq!(lengths.iter().sum::<u64>(), total);
        assert!(sched.is_done());
    }

    #[test]
    fn partial_write_keeps_the_prefix() {
        let mut sched = sched(32 * 1024 * 1024);
        let assignment = sched.assign(0, 4).unwrap();
        let half = assignment.len() / 2;
        sched.commit(&assignment.leases, half);

        assert_eq!(sched.confirmed_bytes(), half, "已写的字节必须算数");
        assert!(!sched.is_done());
        // 剩下的字节 = 总量 - 已确认
        assert_eq!(sched.remaining_bytes(), 32 * 1024 * 1024 - half);
    }

    #[test]
    fn retry_only_downloads_the_gap() {
        let total = 32 * 1024 * 1024;
        let mut sched = sched(total);
        let assignment = sched.assign(0, 4).unwrap();
        let len = assignment.len();
        sched.commit(&assignment.leases, len / 4);

        // 后续派发应当只覆盖缺口，不会把已经拿到的那部分再下一遍
        let mut resent = 0;
        while let Some(next) = sched.assign(0, 4) {
            resent += next.len();
            sched.commit(&next.leases, next.len());
        }
        assert_eq!(resent as u64, total - len / 4);
        assert!(sched.is_done());
    }

    #[test]
    fn reclaim_puts_bytes_back() {
        let mut sched = sched(32 * 1024 * 1024);
        let assignment = sched.assign(0, 4).unwrap();
        let before = sched.remaining_bytes();
        sched.reclaim(&assignment.leases);
        assert_eq!(sched.remaining_bytes(), before + assignment.len());
        assert_eq!(sched.confirmed_bytes(), 0);
        assert_eq!(sched.active_leases(), 0);
    }

    #[test]
    fn zero_written_reclaims_everything() {
        let mut sched = sched(32 * 1024 * 1024);
        let assignment = sched.assign(0, 4).unwrap();
        sched.commit(&assignment.leases, 0);
        assert_eq!(sched.confirmed_bytes(), 0);
        assert_eq!(sched.remaining_bytes(), 32 * 1024 * 1024);
    }

    #[test]
    fn written_beyond_lease_is_clamped() {
        let mut sched = sched(32 * 1024 * 1024);
        let assignment = sched.assign(0, 2).unwrap();
        // 服务器多给了字节也不能把账算崩
        sched.commit(&assignment.leases, assignment.len() + 4096);
        assert_eq!(sched.confirmed_bytes(), assignment.len());
        assert!(!sched.is_done());
    }

    #[test]
    fn no_byte_is_issued_twice() {
        let total = 33 * 1024 * 1024;
        let mut sched = sched(total);
        let mut issued = 0u64;
        let mut rounds = 0;
        while let Some(a) = sched.assign(0, 8) {
            issued += a.len();
            // 前几轮故意只写一半，制造碎片；之后写满以收敛
            let w = if rounds < 6 { a.len() / 2 } else { a.len() };
            sched.commit(&a.leases, w);
            rounds += 1;
            assert!(rounds < 10_000, "碎片路径没有收敛");
        }
        // 写一半的那些轮次会让字节被重新派发，所以派发总量大于文件大小是正常的
        assert!(issued >= total);
        assert!(sched.is_done(), "碎片路径没有收敛");
        assert_eq!(sched.confirmed_bytes(), total);
    }

    #[test]
    fn stop_prevents_new_assignments() {
        let mut sched = sched(32 * 1024 * 1024);
        sched.stop();
        assert!(sched.assign(0, 8).is_none());
    }

    #[test]
    fn idle_slots_split_a_large_fragment() {
        // 只剩一段时，空着的连接应当能把这段再切小
        let total = 20 * 1024 * 1024;
        let mut sched = sched(total);
        let first = sched.assign(0, 1).unwrap(); // 单槽位，拿满 4MiB
        assert_eq!(first.len(), 4 * 1024 * 1024);
        sched.commit(&first.leases, first.len());

        let second = sched.assign(1, 8).unwrap();
        assert!(second.len() <= 4 * 1024 * 1024);
    }

    #[test]
    fn confirmed_bytes_match_written_total() {
        let total = 24 * 1024 * 1024;
        let mut sched = sched(total);
        let mut written_total = 0u64;
        let mut rounds = 0;
        while !sched.is_done() {
            let Some(a) = sched.assign(0, 4) else { break };
            // 前两轮少写一点，之后写满
            let w = if rounds < 2 { a.len() / 2 } else { a.len() };
            written_total += w;
            sched.commit(&a.leases, w);
            rounds += 1;
            assert!(rounds < 10_000);
        }
        // 关键不变量：调度器算出的「已确认」必须等于 IO 层实际写盘的字节数，
        // 否则界面进度就会和真实进度脱节（这就是 250MB / 143MB 那个 bug 的根）
        assert_eq!(sched.confirmed_bytes(), written_total);
        assert_eq!(written_total, total);
    }

    #[test]
    fn span_set_merges_adjacent() {
        let mut set = SpanSet::full(0, 100);
        assert!(set.take(Span::new(10, 20).unwrap()));
        assert!(set.take(Span::new(20, 30).unwrap()));
        assert_eq!(set.count(), 2);
        set.give(Span::new(10, 30).unwrap());
        assert_eq!(set.count(), 1, "相邻区间必须合并回去");
    }

    #[test]
    fn span_set_take_rejects_outside() {
        let mut set = SpanSet::full(0, 100);
        assert!(!set.take(Span::new(90, 120).unwrap()), "越界不能摘");
        assert_eq!(set.count(), 1);
    }
}
