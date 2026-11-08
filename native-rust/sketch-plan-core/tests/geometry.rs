use fusion_sketch_plan_core::*;
use serde_json::json;

fn run(item: Primitive) -> PreparedPlan {
    prepare(&Request { items: vec![item] }).unwrap()
}
fn line(a: [f64; 3], b: [f64; 3]) -> Primitive {
    Primitive::Line {
        start: a.into(),
        end: b.into(),
        construction: None,
    }
}
fn polygon(sides: usize) -> Primitive {
    Primitive::Polygon {
        center: [0., 0.].into(),
        radius: 10.,
        sides,
        angle: 0.,
        construction: None,
    }
}
fn spline(count: usize, closed: bool) -> Primitive {
    Primitive::Spline {
        points: (0..count)
            .map(|i| [i as f64, (i % 2) as f64].into())
            .collect(),
        closed,
        construction: None,
    }
}
fn near(a: [f64; 3], b: [f64; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() < 1e-12, "{a:?} vs {b:?}");
    }
}
fn endpoints(g: &Geometry) -> ([f64; 3], [f64; 3]) {
    match g {
        Geometry::Line { start_cm, end_cm }
        | Geometry::Arc {
            start_cm, end_cm, ..
        } => (*start_cm, *end_cm),
        _ => panic!("not a segment"),
    }
}
fn chain(plan: &PreparedPlan) {
    let entities = &plan.items()[0].entities;
    for i in 0..entities.len() {
        assert_eq!(
            endpoints(&entities[i].geometry).1,
            endpoints(&entities[(i + 1) % entities.len()].geometry).0
        );
    }
}

#[test]
fn point_conversion_preserves_three_dimensions_and_signed_zero() {
    assert_eq!(point_cm([25., -30.].into()).unwrap(), [2.5, -3., 0.]);
    let out = point_cm([-0., 20., -45.].into()).unwrap();
    assert_eq!(out[0].to_bits(), (-0f64).to_bits());
    assert_eq!([out[1], out[2]], [2., -4.5]);
    let p = run(Primitive::Point {
        position: [25., -30.].into(),
    });
    assert_eq!(
        p.items()[0].entities[0].geometry,
        Geometry::Point {
            position_cm: [2.5, -3., 0.]
        }
    );
    assert_eq!(p.items()[0].entities[0].construction, None);
}
#[test]
fn lines_keep_order_and_support_supplied_3d_points_without_plane_claim() {
    let p = run(line([10., 20., 30.], [-40., 60., 80.]));
    assert_eq!(
        endpoints(&p.items()[0].entities[0].geometry),
        ([1., 2., 3.], [-4., 6., 8.])
    );
    assert_eq!(p.entity_count(), 1);
    assert_eq!(p.stored_point_count(), 2);
}
#[test]
fn rectangle_has_four_exact_shared_axis_aligned_corners() {
    for (start, end, expected) in [
        (
            [10., 20., 30.],
            [40., 60., 30.],
            [[1., 2., 3.], [4., 2., 3.], [4., 6., 3.], [1., 6., 3.]],
        ),
        (
            [40., 60., 30.],
            [10., 20., 30.],
            [[4., 6., 3.], [1., 6., 3.], [1., 2., 3.], [4., 2., 3.]],
        ),
    ] {
        let p = run(Primitive::Rectangle {
            start: start.into(),
            end: end.into(),
            construction: Some(true),
        });
        chain(&p);
        assert_eq!(p.entity_count(), 4);
        assert_eq!(p.stored_point_count(), 8);
        for (e, want) in p.items()[0].entities.iter().zip(expected) {
            assert_eq!(endpoints(&e.geometry).0, want);
            assert_eq!(e.construction, Some(true));
        }
    }
}
#[test]
fn circle_uses_radius_not_diameter_and_retains_explicit_false() {
    let p = run(Primitive::Circle {
        center: [20., -40., 60.].into(),
        radius: 25.,
        construction: Some(false),
    });
    assert_eq!(
        p.items()[0].entities[0],
        Entity {
            geometry: Geometry::Circle {
                center_cm: [2., -4., 6.],
                radius_cm: 2.5
            },
            construction: Some(false)
        }
    );
}
#[test]
fn signed_arc_sweeps_have_hand_computed_quarter_and_half_turn_endpoints() {
    for (degrees, end) in [
        (90., [1., 1., 3.]),
        (-90., [1., -3., 3.]),
        (180., [-1., -1., 3.]),
    ] {
        let p = run(Primitive::Arc {
            center: [10., -10., 30.].into(),
            start: [30., -10., 30.].into(),
            sweep: degrees,
            construction: None,
        });
        let Geometry::Arc {
            center_cm,
            start_cm,
            end_cm,
            sweep_radians,
        } = &p.items()[0].entities[0].geometry
        else {
            panic!()
        };
        assert_eq!(*center_cm, [1., -1., 3.]);
        assert_eq!(*start_cm, [3., -1., 3.]);
        near(*end_cm, end);
        assert!((*sweep_radians - degrees / 180. * std::f64::consts::PI).abs() < 1e-15);
    }
}
#[test]
fn triangle_vertices_match_thirty_degree_hand_calculation() {
    let p = run(Primitive::Polygon {
        center: [10., -20., 30.].into(),
        radius: 20.,
        sides: 3,
        angle: 30.,
        construction: None,
    });
    let root = 3f64.sqrt();
    let expected = [[1. + root, -1., 3.], [1. - root, -1., 3.], [1., -4., 3.]];
    chain(&p);
    for (e, want) in p.items()[0].entities.iter().zip(expected) {
        near(endpoints(&e.geometry).0, want);
    }
}
#[test]
fn square_is_circumradius_based_and_angle_wrap_is_explicit() {
    let square = |angle| {
        run(Primitive::Polygon {
            center: [10., 20.].into(),
            radius: 10.,
            sides: 4,
            angle,
            construction: None,
        })
    };
    let p = square(90.);
    let q = square(-270.);
    let r = square(450.);
    assert_eq!(p.items(), q.items());
    assert_eq!(p.items(), r.items());
    for (e, want) in
        p.items()[0]
            .entities
            .iter()
            .zip([[1., 3., 0.], [0., 2., 0.], [1., 1., 0.], [2., 2., 0.]])
    {
        near(endpoints(&e.geometry).0, want);
    }
    chain(&p);
}
#[test]
fn diagonal_slot_matches_three_four_five_perpendicular_and_exact_connectivity() {
    let p = run(Primitive::Slot {
        start: [10., 20., 30.].into(),
        end: [40., 60., 30.].into(),
        width: 20.,
        construction: Some(true),
    });
    let expected = [
        [0.2, 2.6, 3.],
        [3.2, 6.6, 3.],
        [4.8, 5.4, 3.],
        [1.8, 1.4, 3.],
    ];
    chain(&p);
    assert_eq!(p.entity_count(), 4);
    assert_eq!(p.stored_point_count(), 10);
    for (entity, want) in p.items()[0].entities.iter().zip(expected) {
        near(endpoints(&entity.geometry).0, want);
        assert_eq!(entity.construction, Some(true));
    }
    for (index, center) in [(1, [4., 6., 3.]), (3, [1., 2., 3.])] {
        let Geometry::Arc {
            center_cm,
            sweep_radians,
            ..
        } = p.items()[0].entities[index].geometry
        else {
            panic!()
        };
        assert_eq!(center_cm, center);
        assert_eq!(sweep_radians, -std::f64::consts::PI);
    }
}
#[test]
fn horizontal_and_reversed_vertical_slots_keep_width_and_direction() {
    for (start, end, expected) in [
        (
            [0., 0.],
            [40., 0.],
            [[0., 1., 0.], [4., 1., 0.], [4., -1., 0.], [0., -1., 0.]],
        ),
        (
            [0., 40.],
            [0., 0.],
            [[1., 4., 0.], [1., 0., 0.], [-1., 0., 0.], [-1., 4., 0.]],
        ),
    ] {
        let p = run(Primitive::Slot {
            start: start.into(),
            end: end.into(),
            width: 20.,
            construction: None,
        });
        chain(&p);
        for (e, want) in p.items()[0].entities.iter().zip(expected) {
            assert_eq!(endpoints(&e.geometry).0, want);
        }
    }
}
#[test]
fn spline_preserves_fit_point_order_and_closure_intent_without_interpolation() {
    let p = run(Primitive::Spline {
        points: vec![
            [0., 0., 0.].into(),
            [10., 20., 30.].into(),
            [40., -10., 60.].into(),
        ],
        closed: true,
        construction: Some(false),
    });
    assert_eq!(
        p.items()[0].entities[0],
        Entity {
            geometry: Geometry::FittedSpline {
                fit_points_cm: vec![[0., 0., 0.], [1., 2., 3.], [4., -1., 6.]],
                closed: true
            },
            construction: Some(false)
        }
    );
    assert_eq!(p.entity_count(), 1);
    assert_eq!(p.stored_point_count(), 3);
}
#[test]
fn item_grouping_construction_omission_and_input_immutability_are_preserved() {
    let req = Request {
        items: vec![
            Primitive::Point {
                position: [0., 0.].into(),
            },
            polygon(3),
            line([0., 0., 0.], [10., 0., 0.]),
        ],
    };
    let before = req.clone();
    let p = prepare(&req).unwrap();
    assert_eq!(req, before);
    assert_eq!(
        p.items()
            .iter()
            .map(|i| (i.input_index, i.entities.len()))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 3), (2, 1)]
    );
    assert_eq!(p.entity_count(), 5);
    assert_eq!(p.stored_point_count(), 9);
    let v: serde_json::Value = serde_json::from_slice(&p.to_json_bytes().unwrap()).unwrap();
    assert_eq!(v["kind"], "SUPPLIED_SKETCH_GEOMETRY_PLAN");
    assert_eq!(v["units"], "cm_and_radians");
    for field in [
        "success",
        "modelUnchanged",
        "nativeExecutionStarted",
        "transactional",
        "documentId",
    ] {
        assert!(v.get(field).is_none());
    }
}
#[test]
fn coordinate_conversion_rejects_nonfinite_overflow_and_underflow() {
    for v in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        MAX_ABS_MM + 1.,
        f64::from_bits(1),
    ] {
        assert_eq!(millimeters_to_centimeters(v), Err(Error::Numeric));
    }
    assert_eq!(millimeters_to_centimeters(MAX_ABS_MM), Ok(MAX_ABS_CM));
    assert_eq!(millimeters_to_centimeters(-MAX_ABS_MM), Ok(-MAX_ABS_CM));
    for axis in 0..3 {
        let mut p = [0.; 3];
        p[axis] = f64::NAN;
        assert!(point_cm(p.into()).is_err());
    }
}
#[test]
fn zero_tiny_or_collapsed_lines_rectangles_circles_and_slots_are_refused() {
    let items = vec![
        line([0., 0., 0.], [0., 0., 0.]),
        line([0., 0., 0.], [1e-8, 0., 0.]),
        Primitive::Rectangle {
            start: [0., 0.].into(),
            end: [0., 10.].into(),
            construction: None,
        },
        Primitive::Circle {
            center: [0., 0.].into(),
            radius: 0.,
            construction: None,
        },
        Primitive::Circle {
            center: [0., 0.].into(),
            radius: 1e-8,
            construction: None,
        },
        Primitive::Slot {
            start: [0., 0.].into(),
            end: [0., 0.].into(),
            width: 20.,
            construction: None,
        },
        Primitive::Slot {
            start: [0., 0.].into(),
            end: [10., 0.].into(),
            width: 1e-8,
            construction: None,
        },
    ];
    for item in items {
        assert!(prepare(&Request { items: vec![item] }).is_err());
    }
    assert!(prepare(&Request {
        items: vec![line([0., 0., 0.], [2e-7, 0., 0.])]
    })
    .is_ok());
}
#[test]
fn nonpositive_nonfinite_size_fields_and_collapsed_polygon_edges_refuse() {
    for value in [-1., 0., f64::NAN, f64::INFINITY, f64::MAX] {
        for item in [
            Primitive::Circle {
                center: [0., 0.].into(),
                radius: value,
                construction: None,
            },
            Primitive::Polygon {
                center: [0., 0.].into(),
                radius: value,
                sides: 3,
                angle: 0.,
                construction: None,
            },
            Primitive::Slot {
                start: [0., 0.].into(),
                end: [10., 0.].into(),
                width: value,
                construction: None,
            },
        ] {
            assert!(prepare(&Request { items: vec![item] }).is_err());
        }
    }
    let item = Primitive::Polygon {
        center: [0., 0.].into(),
        radius: 1e-6,
        sides: 128,
        angle: 0.,
        construction: None,
    };
    assert_eq!(
        prepare(&Request { items: vec![item] }).unwrap_err(),
        Error::Degenerate
    );
}
#[test]
fn arc_zero_full_turn_nonfinite_tiny_sweep_and_coincident_center_refuse() {
    for sweep in [0., 360., -360., 720., f64::NAN, f64::INFINITY, 1e-20] {
        let item = Primitive::Arc {
            center: [0., 0.].into(),
            start: [10., 0.].into(),
            sweep,
            construction: None,
        };
        assert!(prepare(&Request { items: vec![item] }).is_err(), "{sweep}");
    }
    assert!(prepare(&Request {
        items: vec![Primitive::Arc {
            center: [1., 2.].into(),
            start: [1., 2.].into(),
            sweep: 90.,
            construction: None
        }]
    })
    .is_err());
}
#[test]
fn planar_guards_are_explicit_and_slot_retains_legacy_z_tolerance() {
    for item in [
        Primitive::Rectangle {
            start: [0., 0., 0.].into(),
            end: [10., 10., 1e-12].into(),
            construction: None,
        },
        Primitive::Arc {
            center: [0., 0., 0.].into(),
            start: [10., 0., 1e-12].into(),
            sweep: 90.,
            construction: None,
        },
        Primitive::Slot {
            start: [0., 0., 0.].into(),
            end: [10., 0., 1e-7].into(),
            width: 10.,
            construction: None,
        },
    ] {
        assert_eq!(
            prepare(&Request { items: vec![item] }).unwrap_err(),
            Error::NonCoplanar
        );
    }
    let p = run(Primitive::Slot {
        start: [0., 0., 0.].into(),
        end: [10., 0., 5e-9].into(),
        width: 10.,
        construction: None,
    });
    chain(&p);
    assert_eq!(endpoints(&p.items()[0].entities[0].geometry).1[2], 5e-10);
}
#[test]
fn circle_envelope_and_generated_coordinate_bounds_refuse_large_extents() {
    for item in [
        Primitive::Circle {
            center: [MAX_ABS_MM, 0.].into(),
            radius: 10.,
            construction: None,
        },
        Primitive::Polygon {
            center: [MAX_ABS_MM, 0.].into(),
            radius: 10.,
            sides: 3,
            angle: 90.,
            construction: None,
        },
        Primitive::Arc {
            center: [MAX_ABS_MM - 5., 0.].into(),
            start: [MAX_ABS_MM - 15., 0.].into(),
            sweep: 1.,
            construction: None,
        },
        Primitive::Slot {
            start: [MAX_ABS_MM - 5., 0.].into(),
            end: [MAX_ABS_MM - 15., 0.].into(),
            width: 20.,
            construction: None,
        },
    ] {
        assert_eq!(
            prepare(&Request { items: vec![item] }).unwrap_err(),
            Error::Numeric
        );
    }
}
#[test]
fn side_angle_and_fit_point_caps_include_maximum_and_refuse_extremes() {
    for n in [0, 1, 2, 129, usize::MAX] {
        assert_eq!(
            prepare(&Request {
                items: vec![polygon(n)]
            })
            .unwrap_err(),
            Error::Count
        );
    }
    assert_eq!(run(polygon(128)).entity_count(), 128);
    for angle in [f64::NAN, f64::INFINITY, MAX_ANGLE_DEGREES + 1.] {
        let mut item = polygon(3);
        if let Primitive::Polygon { angle: a, .. } = &mut item {
            *a = angle;
        }
        assert_eq!(
            prepare(&Request { items: vec![item] }).unwrap_err(),
            Error::Numeric
        );
    }
    for n in [0, 1, 129] {
        assert_eq!(
            prepare(&Request {
                items: vec![spline(n, false)]
            })
            .unwrap_err(),
            Error::Count
        );
    }
    assert_eq!(run(spline(128, false)).stored_point_count(), 128);
    assert_eq!(
        prepare(&Request {
            items: vec![spline(2, true)]
        })
        .unwrap_err(),
        Error::Count
    );
}
#[test]
fn spline_adjacent_duplicates_and_repeated_closing_fit_point_are_refused() {
    for (points, closed) in [
        (vec![[0., 0.], [0., 0.]], false),
        (vec![[0., 0.], [10., 20.], [0., 0.]], true),
    ] {
        let item = Primitive::Spline {
            points: points.into_iter().map(Into::into).collect(),
            closed,
            construction: None,
        };
        assert_eq!(
            prepare(&Request { items: vec![item] }).unwrap_err(),
            Error::Degenerate
        );
    }
}
#[test]
fn item_and_expansion_work_budgets_are_checked_without_partial_plan() {
    let point = Primitive::Point {
        position: [0., 0.].into(),
    };
    assert_eq!(
        prepare(&Request { items: vec![] }).unwrap_err(),
        Error::Count
    );
    assert_eq!(
        prepare(&Request {
            items: vec![point.clone(); 128]
        })
        .unwrap()
        .entity_count(),
        128
    );
    assert_eq!(
        prepare(&Request {
            items: vec![point.clone(); 129]
        })
        .unwrap_err(),
        Error::Count
    );
    let mut req = Request {
        items: vec![polygon(128); 32],
    };
    assert_eq!(prepare(&req).unwrap().entity_count(), 4096);
    req.items.push(point.clone());
    assert_eq!(prepare(&req).unwrap_err(), Error::Budget);
    let mut req = Request {
        items: vec![point, line([0., 0., 0.], [0., 0., 0.])],
    };
    assert_eq!(prepare(&req).unwrap_err(), Error::Degenerate);
    req.items.pop();
    assert!(prepare(&req).is_ok());
}
#[test]
fn stored_point_budget_and_tightened_limits_are_inclusive() {
    let req = Request {
        items: vec![spline(128, false); 128],
    };
    let p = prepare(&req).unwrap();
    assert_eq!(p.stored_point_count(), 16384);
    let mut limits = Limits::default();
    limits.max_stored_points -= 1;
    assert_eq!(
        prepare_with_limits(&req, limits).unwrap_err(),
        Error::Budget
    );
    for limits in [
        Limits {
            max_entities: MAX_ENTITIES + 1,
            ..Limits::default()
        },
        Limits {
            max_stored_points: MAX_STORED_POINTS + 1,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: MAX_OUTPUT_BYTES + 1,
            ..Limits::default()
        },
        Limits {
            max_entities: 0,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            prepare_with_limits(
                &Request {
                    items: vec![polygon(3)]
                },
                limits
            )
            .unwrap_err(),
            Error::Budget
        );
    }
}
#[test]
fn output_bytes_are_measured_with_real_serializer_and_never_truncated() {
    let req = Request {
        items: vec![polygon(128), spline(128, true)],
    };
    let p = prepare(&req).unwrap();
    let n = p.to_json_bytes().unwrap().len();
    let limits = Limits {
        max_output_bytes: n,
        ..Limits::default()
    };
    assert_eq!(
        prepare_with_limits(&req, limits)
            .unwrap()
            .to_json_bytes()
            .unwrap()
            .len(),
        n
    );
    assert_eq!(
        prepare_with_limits(
            &req,
            Limits {
                max_output_bytes: n - 1,
                ..limits
            }
        )
        .unwrap_err(),
        Error::Budget
    );
}
#[test]
fn strict_typed_json_matches_all_eight_primitive_shapes_and_defaults() {
    let req: Request = serde_json::from_value(json!({"items":[
        {"type":"point","position":[1,2]}, {"type":"line","start":[0,0],"end":[10,0]},
        {"type":"rectangle","start":[0,0],"end":[10,20],"construction":false},
        {"type":"circle","center":[0,0],"radius":10,"construction":true},
        {"type":"arc","center":[0,0],"start":[10,0],"sweep":-90},
        {"type":"polygon","center":[0,0],"radius":10,"sides":4},
        {"type":"slot","start":[0,0],"end":[10,0],"width":10},
        {"type":"spline","points":[[0,0],[10,10]]}
    ]}))
    .unwrap();
    let p = prepare(&req).unwrap();
    assert_eq!(p.items().len(), 8);
    assert_eq!(
        serde_json::from_slice::<Request>(&serde_json::to_vec(&req).unwrap()).unwrap(),
        req
    );
}
#[test]
fn json_rejects_unknown_fields_boolean_numbers_bad_dimensions_and_null_flags() {
    for item in [
        json!({"type":"point","position":[0,0],"construction":false}),
        json!({"type":"point","position":[true,0]}),
        json!({"type":"point","position":[0]}),
        json!({"type":"point","position":[0,0,0,0]}),
        json!({"type":"circle","center":[0,0],"radius":true}),
        json!({"type":"circle","center":[0,0],"radius":10,"construction":null}),
        json!({"type":"polygon","center":[0,0],"radius":10,"sides":3.5}),
        json!({"type":"polygon","center":[0,0],"radius":10,"sides":true}),
        json!({"type":"spline","points":[[0,0],[1,1]],"closed":null}),
        json!({"type":"spline","points":[[0,0],[1,1]],"mystery":0}),
    ] {
        assert!(serde_json::from_value::<Request>(json!({"items":[item]})).is_err());
    }
    assert!(serde_json::from_value::<Request>(
        json!({"items":[],"documentId":"not native evidence"})
    )
    .is_err());
}
