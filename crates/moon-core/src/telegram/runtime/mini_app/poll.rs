//! Coalescing of identical in-flight Mini App reads before they reach the bounded Work queue.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use crate::telegram::runtime::Work;
use crate::telegram::web::dto::{
    CoinTfDto, CoinTicksDto, CoresDto, LiveTicksDto, NotifyDto, OrdersDto, ReportDto,
    ReportPeriodDto, StrategiesDto, TradeTicksDto, TradesDto,
};
use crate::telegram::web::{MiniAppApiError, MiniAppApiRequest};

/// Route and semantic arguments of one read; transport identity is rechecked by the consumer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PollKey {
    TradeTicks {
        chat: i64,
        core: u64,
        rec_id: i64,
        new_rec_id: i64,
        range: u8,
    },
    OrderTicks {
        chat: i64,
        core: u64,
        uid: u64,
        since_ms: Option<i64>,
    },
    CoinTicks {
        chat: i64,
        core: u64,
        market: String,
        tf: u8,
        since_ms: Option<i64>,
    },
    Report {
        chat: i64,
        period: u8,
    },
    Cores(i64),
    Orders(i64),
    Trades(i64),
    Strategies(i64),
    Notify(i64),
}

/// Stable key for a closed-trade viewport.
fn range_key(range: crate::telegram::web::dto::ChartRangeDto) -> u8 {
    use crate::telegram::web::dto::ChartRangeDto as Range;
    match range {
        Range::Deal => 0,
        Range::Seconds30 => 1,
        Range::Seconds300 => 2,
    }
}

/// Stable key for a market-chart timeframe.
fn timeframe_key(tf: CoinTfDto) -> u8 {
    match tf {
        CoinTfDto::Ticks => 0,
        CoinTfDto::M1 => 1,
        CoinTfDto::M5 => 2,
        CoinTfDto::M15 => 3,
    }
}

/// Stable key for a named report period.
fn period_key(period: ReportPeriodDto) -> u8 {
    match period {
        ReportPeriodDto::Today => 0,
        ReportPeriodDto::Yesterday => 1,
        ReportPeriodDto::Month => 2,
        ReportPeriodDto::LastMonth => 3,
    }
}

/// One typed reply shared by every HTTP waiter of the same read.
enum Poll {
    TradeTicks {
        receiver: Receiver<Result<TradeTicksDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<TradeTicksDto, MiniAppApiError>>>,
    },
    OrderTicks {
        receiver: Receiver<Result<LiveTicksDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<LiveTicksDto, MiniAppApiError>>>,
    },
    CoinTicks {
        receiver: Receiver<Result<CoinTicksDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<CoinTicksDto, MiniAppApiError>>>,
    },
    Report {
        receiver: Receiver<Result<ReportDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<ReportDto, MiniAppApiError>>>,
    },
    Cores {
        receiver: Receiver<Result<CoresDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<CoresDto, MiniAppApiError>>>,
    },
    Orders {
        receiver: Receiver<Result<OrdersDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<OrdersDto, MiniAppApiError>>>,
    },
    Trades {
        receiver: Receiver<Result<TradesDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<TradesDto, MiniAppApiError>>>,
    },
    Strategies {
        receiver: Receiver<Result<StrategiesDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<StrategiesDto, MiniAppApiError>>>,
    },
    Notify {
        receiver: Receiver<Result<NotifyDto, MiniAppApiError>>,
        replies: Vec<SyncSender<Result<NotifyDto, MiniAppApiError>>>,
    },
}

/// A read split for coalescing, or an ordinary request that must keep its own Work item.
enum PollSplit {
    Poll((PollKey, MiniAppApiRequest, Poll)),
    Other(MiniAppApiRequest),
}

impl Poll {
    /// Split a read into its queue item and waiter group; writes remain ordinary Work items.
    fn split(request: MiniAppApiRequest) -> PollSplit {
        match request {
            MiniAppApiRequest::TradeTicks {
                identity,
                chat_id,
                core,
                rec_id,
                new_rec_id,
                range,
                reply,
            } => {
                let key = PollKey::TradeTicks {
                    chat: chat_id,
                    core,
                    rec_id,
                    new_rec_id,
                    range: range_key(range),
                };
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::TradeTicks {
                    identity,
                    chat_id,
                    core,
                    rec_id,
                    new_rec_id,
                    range,
                    reply: shared,
                };
                PollSplit::Poll((
                    key,
                    request,
                    Self::TradeTicks {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::OrderTicks {
                identity,
                chat_id,
                core,
                uid,
                since_ms,
                reply,
            } => {
                let key = PollKey::OrderTicks {
                    chat: chat_id,
                    core,
                    uid,
                    since_ms,
                };
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::OrderTicks {
                    identity,
                    chat_id,
                    core,
                    uid,
                    since_ms,
                    reply: shared,
                };
                PollSplit::Poll((
                    key,
                    request,
                    Self::OrderTicks {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::CoinTicks {
                identity,
                chat_id,
                core,
                market,
                tf,
                since_ms,
                reply,
            } => {
                let key = PollKey::CoinTicks {
                    chat: chat_id,
                    core,
                    market: market.clone(),
                    tf: timeframe_key(tf),
                    since_ms,
                };
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::CoinTicks {
                    identity,
                    chat_id,
                    core,
                    market,
                    tf,
                    since_ms,
                    reply: shared,
                };
                PollSplit::Poll((
                    key,
                    request,
                    Self::CoinTicks {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Report {
                identity,
                chat_id,
                period,
                reply,
            } => {
                let key = PollKey::Report {
                    chat: chat_id,
                    period: period_key(period),
                };
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Report {
                    identity,
                    chat_id,
                    period,
                    reply: shared,
                };
                PollSplit::Poll((
                    key,
                    request,
                    Self::Report {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Cores {
                identity,
                chat_id,
                reply,
            } => {
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Cores {
                    identity,
                    chat_id,
                    reply: shared,
                };
                PollSplit::Poll((
                    PollKey::Cores(chat_id),
                    request,
                    Self::Cores {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Orders {
                identity,
                chat_id,
                reply,
            } => {
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Orders {
                    identity,
                    chat_id,
                    reply: shared,
                };
                PollSplit::Poll((
                    PollKey::Orders(chat_id),
                    request,
                    Self::Orders {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Trades {
                identity,
                chat_id,
                reply,
            } => {
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Trades {
                    identity,
                    chat_id,
                    reply: shared,
                };
                PollSplit::Poll((
                    PollKey::Trades(chat_id),
                    request,
                    Self::Trades {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Strategies {
                identity,
                chat_id,
                reply,
            } => {
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Strategies {
                    identity,
                    chat_id,
                    reply: shared,
                };
                PollSplit::Poll((
                    PollKey::Strategies(chat_id),
                    request,
                    Self::Strategies {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            MiniAppApiRequest::Notify {
                identity,
                chat_id,
                reply,
            } => {
                let (shared, receiver) = sync_channel(1);
                let request = MiniAppApiRequest::Notify {
                    identity,
                    chat_id,
                    reply: shared,
                };
                PollSplit::Poll((
                    PollKey::Notify(chat_id),
                    request,
                    Self::Notify {
                        receiver,
                        replies: vec![reply],
                    },
                ))
            }
            other => PollSplit::Other(other),
        }
    }

    /// Attach the original HTTP waiters of an identical read, not its replacement relay channel.
    fn join(&mut self, poll: Self) {
        match (self, poll) {
            (
                Self::TradeTicks { replies, .. },
                Self::TradeTicks {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::OrderTicks { replies, .. },
                Self::OrderTicks {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::CoinTicks { replies, .. },
                Self::CoinTicks {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Report { replies, .. },
                Self::Report {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Cores { replies, .. },
                Self::Cores {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Orders { replies, .. },
                Self::Orders {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Trades { replies, .. },
                Self::Trades {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Strategies { replies, .. },
                Self::Strategies {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            (
                Self::Notify { replies, .. },
                Self::Notify {
                    replies: incoming, ..
                },
            ) => replies.extend(incoming),
            _ => unreachable!("poll key and request types diverged"),
        }
    }

    /// Whether this group still waits; a completed result is cloned to every remaining waiter.
    fn awaiting(&mut self) -> bool {
        match self {
            Self::TradeTicks { receiver, replies } => awaiting(receiver, replies),
            Self::OrderTicks { receiver, replies } => awaiting(receiver, replies),
            Self::CoinTicks { receiver, replies } => awaiting(receiver, replies),
            Self::Report { receiver, replies } => awaiting(receiver, replies),
            Self::Cores { receiver, replies } => awaiting(receiver, replies),
            Self::Orders { receiver, replies } => awaiting(receiver, replies),
            Self::Trades { receiver, replies } => awaiting(receiver, replies),
            Self::Strategies { receiver, replies } => awaiting(receiver, replies),
            Self::Notify { receiver, replies } => awaiting(receiver, replies),
        }
    }
}

/// Fan out one ready typed result, or retire the group if its queue item was dropped.
fn awaiting<T: Clone>(
    receiver: &Receiver<Result<T, MiniAppApiError>>,
    replies: &mut [SyncSender<Result<T, MiniAppApiError>>],
) -> bool {
    match receiver.try_recv() {
        Ok(result) => {
            for reply in replies {
                let _ = reply.try_send(result.clone());
            }
            false
        }
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => false,
    }
}

/// Distinct reads currently occupying one Work item each, separated by intervening writes.
#[derive(Default)]
pub(super) struct MiniAppPolls {
    pending: HashMap<(u64, PollKey), Poll>,
    write_generation: u64,
}

impl MiniAppPolls {
    /// Relay one request, attaching an identical read instead of consuming another queue slot.
    pub(super) fn relay(&mut self, tx: &SyncSender<Work>, request: MiniAppApiRequest) {
        let (key, request, poll) = match Poll::split(request) {
            PollSplit::Poll(split) => split,
            PollSplit::Other(request) => {
                if tx.try_send(Work::MiniApp(request)).is_ok() {
                    self.write_generation = self.write_generation.wrapping_add(1);
                }
                return;
            }
        };
        let key = (self.write_generation, key);
        if let Some(pending) = self.pending.get_mut(&key) {
            pending.join(poll);
            return;
        }
        if tx.try_send(Work::MiniApp(request)).is_ok() {
            self.pending.insert(key, poll);
        }
    }

    /// Wake within a millisecond while a completed read may be ready for its HTTP waiters.
    pub(super) fn idle_interval(&self, base: std::time::Duration) -> std::time::Duration {
        if self.pending.is_empty() {
            base
        } else {
            base.min(std::time::Duration::from_millis(1))
        }
    }

    /// Release every completed group without blocking the Mini App owner loop.
    pub(super) fn drain(&mut self) {
        self.pending.retain(|_, poll| poll.awaiting());
    }
}

#[cfg(test)]
mod tests;
