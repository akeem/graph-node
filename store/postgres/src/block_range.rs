use diesel::pg::Pg;
use diesel::query_builder::{AstPass, QueryFragment};
use diesel::result::QueryResult;
///! Utilities to deal with block numbers and block ranges
use diesel::serialize::{Output, ToSql};
use diesel::sql_types::{Integer, Range};
use std::io::Write;
use std::ops::{Bound, RangeBounds, RangeFrom};

use graph::prelude::{EventSource, HistoryEvent};

use crate::relational::BLOCK_RANGE;

/// The type we use for block numbers. This has to be a signed integer type
/// since Postgres does not support unsigned integer types. But 2G ought to
/// be enough for everybody
pub type BlockNumber = i32;

pub const BLOCK_NUMBER_MAX: BlockNumber = std::i32::MAX;

/// The range of blocks for which an entity is valid. We need this struct
/// to bind ranges into Diesel queries.
#[derive(Clone, Debug)]
pub struct BlockRange(Bound<BlockNumber>, Bound<BlockNumber>);

// Doing this properly by implementing Clone for Bound is currently
// a nightly-only feature, so we need to work around that
fn clone_bound(bound: Bound<&BlockNumber>) -> Bound<BlockNumber> {
    match bound {
        Bound::Included(nr) => Bound::Included(*nr),
        Bound::Excluded(nr) => Bound::Excluded(*nr),
        Bound::Unbounded => Bound::Unbounded,
    }
}

/// Return the block number contained in the history event. If it is
/// `None` panic because that indicates that we want to perform an
/// operation that does not record history, which should not happen
/// with how we currently use relational schemas
pub(crate) fn block_number(history_event: &Option<&HistoryEvent>) -> BlockNumber {
    match history_event {
        None => panic!("operation that requires a history event did not receive one"),
        Some(HistoryEvent {
            source: EventSource::EthereumBlock(block_ptr),
            ..
        }) => {
            if block_ptr.number < std::i32::MAX as u64 {
                block_ptr.number as i32
            } else {
                panic!(
                    "Block numbers bigger than {} are not supported, but received block number {}",
                    std::i32::MAX,
                    block_ptr.number
                )
            }
        }
    }
}

impl From<RangeFrom<BlockNumber>> for BlockRange {
    fn from(range: RangeFrom<BlockNumber>) -> BlockRange {
        BlockRange(
            clone_bound(range.start_bound()),
            clone_bound(range.end_bound()),
        )
    }
}

impl ToSql<Range<Integer>, Pg> for BlockRange {
    fn to_sql<W: Write>(&self, out: &mut Output<W, Pg>) -> diesel::serialize::Result {
        let pair = (self.0, self.1);
        ToSql::<Range<Integer>, Pg>::to_sql(&pair, out)
    }
}

/// Generate the clause that checks whether `block` is in the block range
/// of an entity
#[derive(Constructor)]
pub struct BlockRangeContainsClause {
    block: BlockNumber,
}

impl QueryFragment<Pg> for BlockRangeContainsClause {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        out.push_identifier(BLOCK_RANGE)?;
        out.push_sql(" @> ");
        out.push_bind_param::<Integer, _>(&self.block)
    }
}
