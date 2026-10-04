//! Flowchart edge curve selection and bounds.
//!
//! This logic is shared between flowchart SVG emission and viewBox computation, and mirrors the
//! behavior of upstream Mermaid when selecting the D3 curve implementation and when deciding
//! whether curve bounds can be skipped for `basis`.

use super::*;

pub(in crate::svg::parity::flowchart) fn curve_path_d_and_bounds(
    line_data: &[crate::model::LayoutPoint],
    interpolate: &str,
    rounded_radius: f64,
    compact_edge_corners: bool,
    rounded_corner_mask: Option<&[bool]>,
) -> (String, Option<path_bounds::SvgPathBounds>, bool) {
    let curve_is_basis = !matches!(
        interpolate,
        "linear"
            | "natural"
            | "bumpX"
            | "bumpY"
            | "catmullRom"
            | "step"
            | "stepAfter"
            | "stepBefore"
            | "cardinal"
            | "monotoneX"
            | "monotoneY"
            | "rounded"
    );

    if curve_is_basis {
        let (d, raw_pb) = crate::svg::parity::curve::curve_basis_path_d_and_bounds(line_data);
        let d = maybe_close_single_point_path(d, line_data);
        let pb = svg_path_bounds_from_d(&d).or(raw_pb);
        (d, pb, false)
    } else {
        let (d, pb) = match interpolate {
            "linear" => crate::svg::parity::curve::curve_linear_path_d_and_bounds(line_data),
            "natural" => crate::svg::parity::curve::curve_natural_path_d_and_bounds(line_data),
            "bumpX" | "bumpY" => crate::svg::parity::curve::curve_bump_path_d_and_bounds(
                line_data,
                interpolate == "bumpX",
            ),
            "catmullRom" => {
                crate::svg::parity::curve::curve_catmull_rom_path_d_and_bounds(line_data)
            }
            "step" => crate::svg::parity::curve::curve_step_path_d_and_bounds(line_data),
            "stepAfter" => crate::svg::parity::curve::curve_step_after_path_d_and_bounds(line_data),
            "stepBefore" => {
                crate::svg::parity::curve::curve_step_before_path_d_and_bounds(line_data)
            }
            "cardinal" => {
                crate::svg::parity::curve::curve_cardinal_path_d_and_bounds(line_data, 0.0)
            }
            "monotoneX" => {
                crate::svg::parity::curve::curve_monotone_path_d_and_bounds(line_data, false)
            }
            "monotoneY" => {
                crate::svg::parity::curve::curve_monotone_path_d_and_bounds(line_data, true)
            }
            "rounded" => crate::svg::parity::curve::curve_rounded_path_d_and_bounds(
                line_data,
                rounded_radius,
                compact_edge_corners,
                rounded_corner_mask,
            ),
            // Unknown curve names fall back to Mermaid's historical `basis` behavior.
            _ => crate::svg::parity::curve::curve_basis_path_d_and_bounds(line_data),
        };

        let d = maybe_close_single_point_path(d, line_data);
        let pb = svg_path_bounds_from_d(&d).or(pb);
        (d, pb, false)
    }
}

fn maybe_close_single_point_path(d: String, line_data: &[crate::model::LayoutPoint]) -> String {
    if line_data.len() == 1 && !d.ends_with('Z') {
        let mut d = d;
        d.push('Z');
        d
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_d3_curve_names_retain_reference_paths_and_finite_bounds() {
        // D3-shape 3.2.0: line().curve(curveName)([[0,0],[10,20],[30,5],[60,30]]).
        // Independent dispatch expectations. For step, retain the input vertices on the
        // same D3 horizontal segments; these extra collinear points do not change geometry.
        let points = [(0.0, 0.0), (10.0, 20.0), (30.0, 5.0), (60.0, 30.0)]
            .map(|(x, y)| crate::model::LayoutPoint { x, y });
        for (name, expected) in [
            (
                "basis",
                "M0,0L1.667,3.333C3.333,6.667,6.667,13.333,11.667,14.167C16.667,15,23.333,10,31.667,11.667C40,13.333,50,21.667,55,25.833L60,30",
            ),
            ("linear", "M0,0L10,20L30,5L60,30"),
            (
                "natural",
                "M0,0C2.667,10.667,5.333,21.333,10,20C14.667,18.667,21.333,5.333,30,5C38.667,4.667,49.333,17.333,60,30",
            ),
            (
                "bumpX",
                "M0,0C5,0,5,20,10,20C20,20,20,5,30,5C45,5,45,30,60,30",
            ),
            (
                "bumpY",
                "M0,0C0,10,10,10,10,20C10,12.5,30,12.5,30,5C30,17.5,60,17.5,60,30",
            ),
            (
                "catmullRom",
                "M0,0C0,0,5.222,18.872,10,20C15.052,21.193,22.74,4.814,30,5C39.073,5.233,60,30,60,30",
            ),
            ("step", "M0,0L5,0L5,20L10,20L20,20L20,5L30,5L45,5L45,30L60,30"),
            ("stepAfter", "M0,0L10,0L10,20L30,20L30,5L60,5L60,30"),
            ("stepBefore", "M0,0L0,20L10,20L10,5L30,5L30,30L60,30"),
            (
                "cardinal",
                "M0,0C0,0,5,19.167,10,20C15,20.833,21.667,3.333,30,5C38.333,6.667,60,30,60,30",
            ),
            (
                "monotoneX",
                "M0,0C3.333,10,6.667,20,10,20C16.667,20,23.333,5,30,5C40,5,50,17.5,60,30",
            ),
            (
                "monotoneY",
                "M0,0C5,6.667,10,13.333,10,20C10,15,30,10,30,5C30,13.333,45,21.667,60,30",
            ),
        ] {
            let (path, bounds, _) = curve_path_d_and_bounds(&points, name, 12.0, false, None);
            assert_eq!(path, expected, "{name}");
            let b = bounds.unwrap();
            assert!(
                [b.min_x, b.min_y, b.max_x, b.max_y]
                    .into_iter()
                    .all(f64::is_finite)
            );
            assert!(b.min_x <= 0.0 && b.min_y <= 0.0 && b.max_x >= 60.0 && b.max_y >= 30.0);
            assert_eq!(curve_path_d_and_bounds(&[], name, 12.0, false, None).0, "");
            assert_eq!(
                curve_path_d_and_bounds(&points[..1], name, 12.0, false, None).0,
                "M0,0Z"
            );
        }
    }

    #[test]
    fn bump_curves_use_the_requested_axis_and_keep_exact_bounds() {
        let points = [
            crate::model::LayoutPoint { x: 0.0, y: 0.0 },
            crate::model::LayoutPoint { x: 10.0, y: 20.0 },
            crate::model::LayoutPoint { x: 30.0, y: 5.0 },
        ];
        for (curve, expected) in [
            ("bumpX", "M0,0C5,0,5,20,10,20C20,20,20,5,30,5"),
            ("bumpY", "M0,0C0,10,10,10,10,20C10,12.5,30,12.5,30,5"),
        ] {
            let (path, bounds, _) = curve_path_d_and_bounds(&points, curve, 0.0, false, None);
            assert_eq!(path, expected);
            let bounds = bounds.unwrap();
            assert_eq!(
                (bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y),
                (0.0, 0.0, 30.0, 20.0)
            );
            assert_eq!(curve_path_d_and_bounds(&[], curve, 0.0, false, None).0, "");
            assert_eq!(
                curve_path_d_and_bounds(&points[..1], curve, 0.0, false, None).0,
                "M0,0Z"
            );
        }
    }

    #[test]
    fn maybe_close_single_point_path_appends_z_once() {
        let line_data = vec![crate::model::LayoutPoint { x: 1.0, y: 2.0 }];

        assert_eq!(
            maybe_close_single_point_path("M1,2".to_string(), &line_data),
            "M1,2Z"
        );
        assert_eq!(
            maybe_close_single_point_path("M1,2Z".to_string(), &line_data),
            "M1,2Z"
        );
    }

    #[test]
    fn maybe_close_single_point_path_preserves_multi_point_paths() {
        let line_data = vec![
            crate::model::LayoutPoint { x: 1.0, y: 2.0 },
            crate::model::LayoutPoint { x: 3.0, y: 4.0 },
        ];

        assert_eq!(
            maybe_close_single_point_path("M1,2L3,4".to_string(), &line_data),
            "M1,2L3,4"
        );
    }

    #[test]
    fn compact_rounded_corners_do_not_overrun_shallow_elk_endpoint_turns() {
        let line_data = vec![
            crate::model::LayoutPoint {
                x: 130.484,
                y: 214.303,
            },
            crate::model::LayoutPoint {
                x: 135.784,
                y: 230.203,
            },
            crate::model::LayoutPoint {
                x: 135.784,
                y: 250.203,
            },
            crate::model::LayoutPoint {
                x: 260.023,
                y: 250.203,
            },
        ];

        let (parity, _, _) = curve_path_d_and_bounds(&line_data, "rounded", 12.0, false, None);
        let (compact, _, _) = curve_path_d_and_bounds(&line_data, "rounded", 12.0, true, None);

        assert!(
            parity.starts_with("M130.484,214.303L133.134,222.253Q135.784,230.203 135.784,238.583"),
            "expected the Mermaid-compatible half-segment cut: {parity}"
        );
        assert!(
            compact.starts_with("M130.484,214.303L135.168,228.356Q135.784,230.203 135.784,232.15"),
            "expected the compact tangent-length cut: {compact}"
        );

        let corner_mask = [true, false, true, true];
        let (adapter_linear, _, _) =
            curve_path_d_and_bounds(&line_data, "rounded", 12.0, true, Some(&corner_mask));
        assert!(
            adapter_linear
                .starts_with("M130.484,214.303L135.784,230.203L135.784,240.203Q135.784,250.203"),
            "expected the endpoint adapter to stay linear while the ELK bend remains rounded: {adapter_linear}"
        );
    }
}
