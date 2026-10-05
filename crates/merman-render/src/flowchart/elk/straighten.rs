//! Mermaid 12 ELK `straightenEdgeTerminals`: move a short channel step, never its ports.
use super::{
    ElkOperationWorkControl, LayoutEdge, LayoutPoint, Result, charge_adapter_work,
    checked_adapter_add, checked_adapter_mul,
};

const JOG_EPS: f64 = 0.01;
const TERMINAL_RUN_MAX: f64 = 30.0;
const TERMINAL_JOG_MAX: f64 = 16.0;

fn axis(a: &LayoutPoint, b: &LayoutPoint) -> Option<bool> {
    let dx = (b.x - a.x).abs();
    let dy = (b.y - a.y).abs();
    if dx > JOG_EPS && dy <= JOG_EPS {
        Some(true)
    } else if dy > JOG_EPS && dx <= JOG_EPS {
        Some(false)
    } else {
        None
    }
}

fn straighten_front(points: &mut Vec<LayoutPoint>) -> bool {
    if points.len() < 5 {
        return false;
    }
    let (p0, p1, p2, p3) = (&points[0], &points[1], &points[2], &points[3]);
    let Some(horizontal) = axis(p0, p1) else {
        return false;
    };
    if axis(p2, p3) != Some(horizontal)
        || axis(p1, p2) != Some(!horizontal)
        || (p1.x - p0.x).hypot(p1.y - p0.y) > TERMINAL_RUN_MAX
    {
        return false;
    }
    let (jog, forward, row) = if horizontal {
        (
            (p2.y - p1.y).abs(),
            (p1.x - p0.x).signum() == (p3.x - p2.x).signum(),
            p0.y,
        )
    } else {
        (
            (p2.x - p1.x).abs(),
            (p1.y - p0.y).signum() == (p3.y - p2.y).signum(),
            p0.x,
        )
    };
    if !(JOG_EPS..=TERMINAL_JOG_MAX).contains(&jog) || !forward {
        return false;
    }
    let mut last = 3;
    while last + 1 < points.len() && axis(&points[last], &points[last + 1]) == Some(horizontal) {
        last += 1;
    }
    if last == points.len() - 1 {
        return false;
    }
    for point in &mut points[2..=last] {
        if horizontal {
            point.y = row;
        } else {
            point.x = row;
        }
    }
    points.drain(1..3);
    true
}

fn candidate(points: &[LayoutPoint]) -> Option<Vec<LayoutPoint>> {
    let mut points = points.to_vec();
    let start = straighten_front(&mut points);
    points.reverse();
    let end = straighten_front(&mut points);
    points.reverse();
    (start || end).then_some(points)
}

fn crossings(a: &[LayoutPoint], b: &[LayoutPoint]) -> usize {
    let side = |o: &LayoutPoint, p: &LayoutPoint, q: &LayoutPoint| {
        (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x)
    };
    let opposite = |a: f64, b: f64| (a > 0.0 && b < 0.0) || (a < 0.0 && b > 0.0);
    a.windows(2)
        .map(|a| {
            b.windows(2)
                .filter(|b| {
                    opposite(side(&b[0], &b[1], &a[0]), side(&b[0], &b[1], &a[1]))
                        && opposite(side(&a[0], &a[1], &b[0]), side(&a[0], &a[1], &b[1]))
                })
                .count()
        })
        .sum()
}

pub(super) fn straighten_edge_terminals(
    edges: &mut [LayoutEdge],
    work: &mut Option<&mut ElkOperationWorkControl>,
) -> Result<()> {
    charge_adapter_work(work, edges.len())?;
    for index in 0..edges.len() {
        let original = &edges[index].points;
        if original.len() < 5 {
            continue;
        }
        // Clone, two end scans/reversals and compaction are linear in this route.
        charge_adapter_work(work, checked_adapter_mul(work, original.len(), 6)?)?;
        let Some(candidate) = candidate(original) else {
            continue;
        };
        let mut before = 0;
        let mut after = 0;
        charge_adapter_work(work, edges.len())?;
        // ponytail: pairwise segment comparisons are budgeted; use a spatial index if dense routes hit the work limit.
        for (other, edge) in edges.iter().enumerate() {
            if other == index || edge.points.len() < 2 {
                continue;
            }
            let pairs = checked_adapter_mul(
                work,
                checked_adapter_add(work, original.len() - 1, candidate.len() - 1)?,
                edge.points.len() - 1,
            )?;
            charge_adapter_work(work, pairs)?;
            before = checked_adapter_add(work, before, crossings(original, &edge.points))?;
            after = checked_adapter_add(work, after, crossings(&candidate, &edge.points))?;
        }
        if after <= before {
            edges[index].points = candidate;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{OperationWorkMeter, RenderResourcePolicy};
    use std::sync::Arc;

    fn points(values: &[(f64, f64)]) -> Vec<LayoutPoint> {
        values.iter().map(|&(x, y)| LayoutPoint { x, y }).collect()
    }
    fn coordinates(points: &[LayoutPoint]) -> Vec<(f64, f64)> {
        points.iter().map(|p| (p.x, p.y)).collect()
    }
    fn edge(values: &[(f64, f64)]) -> LayoutEdge {
        serde_json::from_value(serde_json::json!({"id":"edge", "from":"a", "to":"b", "points":points(values), "label":null})).unwrap()
    }

    #[test]
    fn moves_the_whole_channel_at_either_end_without_moving_ports() {
        let original = [
            (0., 0.),
            (20., 0.),
            (20., 8.),
            (60., 8.),
            (100., 8.),
            (100., 40.),
        ];
        let expected = [(0., 0.), (60., 0.), (100., 0.), (100., 40.)];
        for swap in [false, true] {
            for sign in [-1., 1.] {
                for reverse in [false, true] {
                    let transform = |input: &[(f64, f64)]| {
                        let mut out: Vec<_> = input
                            .iter()
                            .map(|&(x, y)| {
                                if swap {
                                    (sign * y, sign * x)
                                } else {
                                    (sign * x, sign * y)
                                }
                            })
                            .collect();
                        if reverse {
                            out.reverse();
                        }
                        out
                    };
                    assert_eq!(
                        coordinates(&candidate(&points(&transform(&original))).unwrap()),
                        transform(&expected)
                    );
                }
            }
        }
        let both = points(&[
            (0., 0.),
            (20., 0.),
            (20., 8.),
            (100., 8.),
            (100., 60.),
            (108., 60.),
            (108., 80.),
        ]);
        assert_eq!(
            coordinates(&candidate(&both).unwrap()),
            [(0., 0.), (108., 0.), (108., 80.)]
        );
    }

    #[test]
    fn respects_short_step_thresholds_real_turns_and_the_far_port() {
        for (run, jog, changes) in [
            (30., 16., true),
            (30.01, 8., false),
            (20., 16.01, false),
            (20., 0.01, false),
            (20., 0.011, true),
        ] {
            let route = points(&[(0., 0.), (run, 0.), (run, jog), (100., jog), (100., 60.)]);
            assert_eq!(candidate(&route).is_some(), changes, "run={run}, jog={jog}");
        }
        for route in [
            vec![],
            vec![(0., 0.)],
            vec![(0., 0.), (20., 0.), (20., 8.), (100., 8.)],
            vec![(0., 0.), (20., 0.), (20., 8.), (100., 8.), (200., 8.)],
            vec![(0., 0.), (20., 0.), (20., 8.), (10., 8.), (10., 60.)],
            vec![(0., 0.), (20., 2.), (20., 8.), (100., 8.), (100., 60.)],
        ] {
            assert!(candidate(&points(&route)).is_none(), "{route:?}");
        }
    }

    #[test]
    fn rejects_new_crossings_including_diagonals_but_not_endpoint_touches() {
        let route = [(0., 0.), (20., 0.), (20., 8.), (100., 8.), (100., 40.)];
        for (other, accepted) in [
            ([(60., -5.), (60., 5.)], false),
            ([(50., -5.), (70., 5.)], false),
            ([(60., 0.), (60., 5.)], true),
            ([(60., -5.), (60., 15.)], true),
        ] {
            let mut edges = vec![edge(&route), edge(&other)];
            straighten_edge_terminals(&mut edges, &mut None).unwrap();
            assert_eq!(edges[0].points.len(), if accepted { 3 } else { 5 });
            assert_eq!(coordinates(&edges[1].points), other);
        }
    }

    #[test]
    fn charges_crossing_comparisons_before_work_and_honors_cancellation() {
        // Two route rows, 6*5 candidate units, two comparison rows, (4+2)*1 pairs.
        const WORK: usize = 40;
        for limit in [WORK - 1, WORK] {
            let mut edges = vec![
                edge(&[(0., 0.), (20., 0.), (20., 8.), (100., 8.), (100., 40.)]),
                edge(&[(200., 0.), (200., 40.)]),
            ];
            let policy = RenderResourcePolicy::unbounded_for_trusted_input()
                .with_limit(crate::ResourceLimitId::MaxLayoutWorkUnits, limit)
                .unwrap();
            let meter = Arc::new(OperationWorkMeter::new(policy));
            let mut control = ElkOperationWorkControl::new(meter);
            let result = straighten_edge_terminals(&mut edges, &mut Some(&mut control));
            assert_eq!(result.is_ok(), limit == WORK);
            assert_eq!(edges[0].points.len(), if limit == WORK { 3 } else { 5 });
            assert_eq!(
                control.adapter_work(),
                if limit == WORK { WORK } else { 34 }
            );
        }
        let cancel = merman_core::OperationControl::new();
        cancel.cancel();
        let meter = Arc::new(OperationWorkMeter::new_with_control(
            RenderResourcePolicy::unbounded_for_trusted_input(),
            cancel,
        ));
        let mut control = ElkOperationWorkControl::new(meter);
        let mut edges = vec![edge(&[
            (0., 0.),
            (20., 0.),
            (20., 8.),
            (100., 8.),
            (100., 40.),
        ])];
        assert!(straighten_edge_terminals(&mut edges, &mut Some(&mut control)).is_err());
        assert_eq!(control.adapter_work(), 0);
        assert_eq!(edges[0].points.len(), 5);
    }
}
