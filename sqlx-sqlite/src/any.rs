use crate::{
    Either, Sqlite, SqliteArgumentValue, SqliteArguments, SqliteColumn, SqliteConnectOptions,
    SqliteConnection, SqliteQueryResult, SqliteRow, SqliteTransactionManager, SqliteTypeInfo,
};
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use futures_util::{StreamExt, TryFutureExt, TryStreamExt};
use std::borrow::Cow;
use std::sync::Arc;

use sqlx_core::any::{
    Any, AnyArguments, AnyColumn, AnyConnectOptions, AnyConnectionBackend, AnyQueryResult, AnyRow,
    AnyStatement, AnyTypeInfo, AnyTypeInfoKind, AnyValue, AnyValueKind,
};

use crate::type_info::DataType;
use sqlx_core::any::driver::AnyDriver;
use sqlx_core::column::Column;
use sqlx_core::connection::{ConnectOptions, Connection};
use sqlx_core::database::Database;
use sqlx_core::describe::Describe;
use sqlx_core::executor::Executor;
use sqlx_core::ext::ustr::UStr;
use sqlx_core::row::Row;
use sqlx_core::transaction::TransactionManager;

sqlx_core::declare_driver_with_optional_migrate!(DRIVER = Sqlite);

impl AnyConnectionBackend for SqliteConnection {
    fn name(&self) -> &str {
        <Sqlite as Database>::NAME
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, sqlx_core::Result<()>> {
        Connection::close(*self)
    }

    fn close_hard(self: Box<Self>) -> BoxFuture<'static, sqlx_core::Result<()>> {
        Connection::close_hard(*self)
    }

    fn ping(&mut self) -> BoxFuture<'_, sqlx_core::Result<()>> {
        Connection::ping(self)
    }

    fn begin(&mut self) -> BoxFuture<'_, sqlx_core::Result<()>> {
        SqliteTransactionManager::begin(self)
    }

    fn commit(&mut self) -> BoxFuture<'_, sqlx_core::Result<()>> {
        SqliteTransactionManager::commit(self)
    }

    fn rollback(&mut self) -> BoxFuture<'_, sqlx_core::Result<()>> {
        SqliteTransactionManager::rollback(self)
    }

    fn start_rollback(&mut self) {
        SqliteTransactionManager::start_rollback(self)
    }

    fn flush(&mut self) -> BoxFuture<'_, sqlx_core::Result<()>> {
        Connection::flush(self)
    }

    fn should_flush(&self) -> bool {
        Connection::should_flush(self)
    }

    fn fetch_many<'q>(
        &'q mut self,
        query: &'q str,
        arguments: Option<AnyArguments<'q>>,
    ) -> BoxStream<'q, sqlx_core::Result<Either<AnyQueryResult, AnyRow>>> {
        let persistent = arguments.is_some();
        let args = arguments.map(map_arguments);

        Box::pin(
            self.worker
                .execute(query, args, self.row_channel_size, persistent)
                .map_ok(flume::Receiver::into_stream)
                .try_flatten_stream()
                .map(
                    move |res: sqlx_core::Result<Either<SqliteQueryResult, SqliteRow>>| match res? {
                        Either::Left(result) => Ok(Either::Left(map_result(result))),
                        Either::Right(row) => Ok(Either::Right(AnyRow::try_from(&row)?)),
                    },
                ),
        )
    }

    fn fetch_optional<'q>(
        &'q mut self,
        query: &'q str,
        arguments: Option<AnyArguments<'q>>,
    ) -> BoxFuture<'q, sqlx_core::Result<Option<AnyRow>>> {
        let persistent = arguments.is_some();
        let args = arguments.map(map_arguments);

        Box::pin(async move {
            let mut stream = self
                .worker
                .execute(query, args, self.row_channel_size, persistent)
                .map_ok(flume::Receiver::into_stream)
                .await?;
            futures_util::pin_mut!(stream);

            if let Some(Either::Right(row)) = stream.try_next().await? {
                return Ok(Some(AnyRow::try_from(&row)?));
            }

            Ok(None)
        })
    }

    fn prepare_with<'c, 'q: 'c>(
        &'c mut self,
        sql: &'q str,
        _parameters: &[AnyTypeInfo],
    ) -> BoxFuture<'c, sqlx_core::Result<AnyStatement<'q>>> {
        Box::pin(async move {
            let statement = Executor::prepare_with(self, sql, &[]).await?;
            AnyStatement::try_from_statement(sql, &statement, statement.column_names.clone())
        })
    }

    fn describe<'q>(&'q mut self, sql: &'q str) -> BoxFuture<'q, sqlx_core::Result<Describe<Any>>> {
        Box::pin(async move { Executor::describe(self, sql).await?.try_into_any() })
    }
}

impl<'a> TryFrom<&'a SqliteTypeInfo> for AnyTypeInfo {
    type Error = sqlx_core::Error;

    fn try_from(sqlite_type: &'a SqliteTypeInfo) -> Result<Self, Self::Error> {
        Ok(AnyTypeInfo {
            kind: match &sqlite_type.0 {
                DataType::Null => AnyTypeInfoKind::Null,
                DataType::Int => AnyTypeInfoKind::Integer,
                DataType::Int64 => AnyTypeInfoKind::BigInt,
                DataType::Float => AnyTypeInfoKind::Double,
                DataType::Blob => AnyTypeInfoKind::Blob,
                DataType::Text => AnyTypeInfoKind::Text,
                _ => {
                    return Err(sqlx_core::Error::AnyDriverError(
                        format!(
                            "Any driver does not support the SQLite type {:?}",
                            sqlite_type
                        )
                        .into(),
                    ))
                }
            },
        })
    }
}

impl<'a> TryFrom<&'a SqliteColumn> for AnyColumn {
    type Error = sqlx_core::Error;

    fn try_from(col: &'a SqliteColumn) -> Result<Self, Self::Error> {
        let type_info =
            AnyTypeInfo::try_from(&col.type_info).map_err(|e| sqlx_core::Error::ColumnDecode {
                index: col.name.to_string(),
                source: e.into(),
            })?;

        Ok(AnyColumn {
            ordinal: col.ordinal,
            name: col.name.clone(),
            type_info,
        })
    }
}

impl<'a> TryFrom<&'a SqliteRow> for AnyRow {
    type Error = sqlx_core::Error;

    fn try_from(row: &'a SqliteRow) -> Result<Self, Self::Error> {
        AnyRow::map_from(row, row.column_names.clone())
    }
}

impl<'a> TryFrom<&'a AnyConnectOptions> for SqliteConnectOptions {
    type Error = sqlx_core::Error;

    fn try_from(opts: &'a AnyConnectOptions) -> Result<Self, Self::Error> {
        let mut opts_out = SqliteConnectOptions::from_url(&opts.database_url)?;
        opts_out.log_settings = opts.log_settings.clone();
        Ok(opts_out)
    }
}

/// Instead of `AnyArguments::convert_into()`, we can do a direct mapping and preserve the lifetime.
fn map_arguments(args: AnyArguments<'_>) -> SqliteArguments<'_> {
    SqliteArguments {
        values: args
            .values
            .0
            .into_iter()
            .map(|val| match val {
                AnyValueKind::Null => SqliteArgumentValue::Null,
                AnyValueKind::SmallInt(i) => SqliteArgumentValue::Int(i as i32),
                AnyValueKind::Integer(i) => SqliteArgumentValue::Int(i),
                AnyValueKind::BigInt(i) => SqliteArgumentValue::Int64(i),
                AnyValueKind::Real(r) => SqliteArgumentValue::Double(r as f64),
                AnyValueKind::Double(d) => SqliteArgumentValue::Double(d),
                AnyValueKind::Text(t) => SqliteArgumentValue::Text(t),
                AnyValueKind::Blob(b) => SqliteArgumentValue::Blob(b),
                // AnyValueKind is `#[non_exhaustive]` but we should have covered everything
                _ => unreachable!("BUG: missing mapping for {:?}", val),
            })
            .collect(),
    }
}

fn map_result(res: SqliteQueryResult) -> AnyQueryResult {
    AnyQueryResult {
        rows_affected: res.rows_affected(),
        last_insert_id: None,
    }
}
