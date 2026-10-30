//! What the SigNoz pane shows: the services with their health, one service's page, logs,
//! traces and one trace, and dashboards with what each panel draws. The core keeps each and
//! reads it again on the pace the manifest gives.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sikemux_plugin_signoz_views::{
    Dashboard, DashboardSummary, DashboardViewRequest, LogPage, PanelData, Series, ServiceHealth,
    ServicePage, ServiceRow, ServiceSort, Services, ServicesRequest, Trace, TracePage,
    VolumeBucket,
};

use crate::dashboards::{self, PanelRequest};
use crate::error::SignozResult;
use crate::filter::Scope;
use crate::logs::{self, LogSearch, VolumeQuery};
use crate::query::View;
use crate::services::{self, ServiceQuery, ServiceRequest};
use crate::traces::{self, TraceRequest, TraceSearch};

/// Every service in the environment asked for, or across all of them, sorted, beside the
/// environments SigNoz knows.
pub async fn services_view(data_dir: &Path, request: ServicesRequest) -> SignozResult<Services> {
    let environment = request.scope.environment.clone();
    let every = Scope {
        environment: None,
        ..request.scope
    };
    let health = services::health(data_dir, ServiceQuery { scope: every }).await?;
    let environments: BTreeSet<String> = health
        .iter()
        .filter_map(|row| row.environment.clone())
        .collect();
    let mut rows = merged(&health, environment.as_deref());
    sort(&mut rows, request.sort);
    Ok(Services {
        rows,
        environments: environments.into_iter().collect(),
    })
}

/// One row per service. Across environments the counts add up and the worst p99 stands.
fn merged(health: &[ServiceHealth], environment: Option<&str>) -> Vec<ServiceRow> {
    let mut rows: Vec<ServiceRow> = Vec::new();
    for row in health {
        if environment.is_some_and(|wanted| row.environment.as_deref() != Some(wanted)) {
            continue;
        }
        match rows.iter_mut().find(|kept| kept.service == row.service) {
            Some(kept) => {
                kept.calls += row.calls;
                kept.errors += row.errors;
                kept.error_rate = match kept.calls {
                    0 => 0.,
                    calls => kept.errors as f64 / calls as f64,
                };
                kept.p99_ms = kept.p99_ms.max(row.p99_ms);
            }
            None => rows.push(ServiceRow {
                service: row.service.clone(),
                calls: row.calls,
                errors: row.errors,
                error_rate: row.error_rate,
                p99_ms: row.p99_ms,
            }),
        }
    }
    rows
}

fn sort(rows: &mut [ServiceRow], by: ServiceSort) {
    rows.sort_by(|left, right| match by {
        ServiceSort::Errors => right
            .error_rate
            .total_cmp(&left.error_rate)
            .then(right.errors.cmp(&left.errors))
            .then(right.calls.cmp(&left.calls)),
        ServiceSort::Calls => right.calls.cmp(&left.calls),
        ServiceSort::P99 => right.p99_ms.total_cmp(&left.p99_ms),
        ServiceSort::Name => left
            .service
            .to_lowercase()
            .cmp(&right.service.to_lowercase()),
    });
}

/// One service's health over time, its operations and its errors grouped by message.
pub async fn service_view(data_dir: &Path, scope: Scope) -> SignozResult<ServicePage> {
    let ask = || ServiceRequest {
        scope: scope.clone(),
    };
    let (overview, operations, errors) = tokio::join!(
        services::overview(data_dir, ask()),
        services::operations(data_dir, ask()),
        services::errors(data_dir, ask()),
    );
    Ok(ServicePage {
        overview: overview?,
        operations: operations?,
        errors: errors?,
    })
}

pub async fn logs_view(data_dir: &Path, search: LogSearch) -> SignozResult<LogPage> {
    logs::page(data_dir, &search).await
}

pub async fn volume_view(data_dir: &Path, query: VolumeQuery) -> SignozResult<Vec<VolumeBucket>> {
    logs::volume(data_dir, query).await
}

pub async fn traces_view(data_dir: &Path, search: TraceSearch) -> SignozResult<TracePage> {
    traces::search(data_dir, search).await
}

pub async fn trace_view(data_dir: &Path, mut request: TraceRequest) -> SignozResult<Trace> {
    request.view = View::Pane;
    traces::whole_trace(data_dir, request).await
}

pub async fn dashboards_view(data_dir: &Path) -> SignozResult<Vec<DashboardSummary>> {
    dashboards::list(data_dir).await
}

pub async fn dashboard_view(
    data_dir: &Path,
    request: DashboardViewRequest,
) -> SignozResult<Dashboard> {
    dashboards::fetch(data_dir, &request.id).await
}

/// What a panel draws, its series past the eighth folded into one called Other.
pub async fn panel_view(data_dir: &Path, request: PanelRequest) -> SignozResult<PanelData> {
    Ok(match dashboards::panel(data_dir, request).await? {
        PanelData::Series { series } => PanelData::Series {
            series: folded(series),
        },
        data => data,
    })
}

/// Series that keep their own colour; the smallest of the rest are summed into "Other".
const SERIES_SLOTS: usize = 8;

fn folded(series: Vec<Series>) -> Vec<Series> {
    if series.len() <= SERIES_SLOTS {
        return series;
    }
    let total = |series: &Series| {
        series
            .points
            .iter()
            .map(|(_, value)| value.abs())
            .sum::<f64>()
    };
    let mut ranked: Vec<(usize, Series)> = series.into_iter().enumerate().collect();
    ranked.sort_by(|(_, left), (_, right)| total(right).total_cmp(&total(left)));
    let rest = ranked.split_off(SERIES_SLOTS - 1);
    let mut sums: BTreeMap<u64, f64> = BTreeMap::new();
    for (_, series) in rest {
        for (at, value) in series.points {
            *sums.entry(at).or_default() += value;
        }
    }
    ranked.sort_by_key(|(at, _)| *at);
    let mut kept: Vec<Series> = ranked.into_iter().map(|(_, series)| series).collect();
    kept.push(Series {
        label: "Other".into(),
        points: sums.into_iter().collect(),
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<ServiceHealth> {
        let row = |service: &str, environment: &str, calls, errors, p99_ms| ServiceHealth {
            service: service.into(),
            environment: Some(environment.into()),
            calls,
            errors,
            error_rate: errors as f64 / calls as f64,
            p99_ms,
        };
        vec![
            row("reel-worker", "production", 90, 9, 10.),
            row("reel-worker", "dev", 10, 1, 40.),
            row("api", "dev", 5, 0, 1.),
        ]
    }

    #[test]
    fn adds_a_service_up_across_environments_and_keeps_its_worst_p99() {
        let merged = merged(&rows(), None);
        let worker = merged.iter().find(|row| row.service == "reel-worker");
        assert_eq!(
            worker,
            Some(&ServiceRow {
                service: "reel-worker".into(),
                calls: 100,
                errors: 10,
                error_rate: 0.1,
                p99_ms: 40.,
            })
        );
    }

    #[test]
    fn keeps_only_the_chosen_environment() {
        let services: Vec<String> = merged(&rows(), Some("production"))
            .into_iter()
            .map(|row| row.service)
            .collect();
        assert_eq!(services, ["reel-worker"]);
    }
}
