//! `selection.get`: the selection read back, resolved, without changing anything.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use crate::{EngineError, Session};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn close(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-6)
}

fn close3(v: &Value, want: [f64; 3]) -> bool {
    v.as_array().is_some_and(|a| a.len() == 3 && a.iter().zip(want).all(|(x, w)| close(x, w)))
}

/// A 40 × 30 × 20 box with a Ø10 hole through it at (20, 15).
fn part(s: &mut Session) -> String {
    run(s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(s, "solid.cylinder", json!({"radius": 5, "height": 22, "base": [20, 15, -1], "operation": "cut"}));
    s.model.state().bodies.first().unwrap().name.clone()
}

/// The face of the first body whose surface satisfies `want`, as a pick.
fn face(s: &Session, want: impl Fn(solidcraft_kernel::FaceSurface) -> bool) -> Value {
    let st = s.model.state();
    let b = st.bodies.first().unwrap();
    let m = b.mesh();
    let n = m.tri_face.iter().map(|f| *f as usize + 1).max().unwrap();
    let f = (0..n).find(|&f| solidcraft_kernel::face_surface(&b.body, f).is_some_and(&want)).expect("face");
    let p = m.triangles.iter().zip(&m.tri_face).find(|(_, tf)| **tf as usize == f).and_then(|(t, _)| m.tri(t)).unwrap()[0];
    json!({"type": "face", "body": b.name, "index": f, "point": p})
}

/// The mesh edge all of whose points satisfy `on`, as a pick.
fn edge(s: &Session, on: impl Fn(Vec3) -> bool) -> Value {
    let st = s.model.state();
    let b = st.bodies.first().unwrap();
    let m = b.mesh();
    let i = m.edges.iter().position(|e| e.iter().all(|p| on(*p))).expect("edge");
    let e = &m.edges[i];
    json!({"type": "edge", "body": b.name, "index": i, "point": e[e.len() / 2]})
}

#[test]
fn empty_selection_is_an_empty_list() {
    let mut s = Session::default();
    assert_eq!(run(&mut s, "selection.get", json!({})), json!({"count": 0, "items": []}));
    part(&mut s);
    assert_eq!(run(&mut s, "selection.get", json!({"detail": false})), json!({"count": 0, "items": []}));
}

#[test]
fn faces_edges_and_bodies_are_resolved() {
    let mut s = Session::default();
    let body = part(&mut s);
    let top = face(&s, |f| matches!(f, solidcraft_kernel::FaceSurface::Plane { normal, .. } if normal.z > 0.99));
    let hole = face(&s, |f| matches!(f, solidcraft_kernel::FaceSurface::Cylinder { .. }));
    // The top front edge: y = 0, z = 20, along X.
    let line = edge(&s, |p| p.y.abs() < 1e-9 && (p.z - 20.0).abs() < 1e-9);
    // The hole's rim on the top face.
    let rim = edge(&s, |p| (p.z - 20.0).abs() < 1e-9 && ((p.x - 20.0).hypot(p.y - 15.0) - 5.0).abs() < 1e-6);
    let items = json!([top, hole, line, rim, {"type": "body", "name": body}]);
    run(&mut s, "select.set", json!({ "items": items }));
    let revision = s.revision;
    let (undo, dirty, doc) = (s.undo.len(), s.is_dirty(), s.doc.clone());

    let r = run(&mut s, "selection.get", json!({}));
    assert_eq!(r["count"], 5, "{r}");
    let it = r["items"].as_array().unwrap();

    let top = &it[0];
    assert_eq!(top["kind"], "face");
    assert_eq!(top["type"], "face", "items keep the select.set form");
    assert_eq!(top["body"], json!(body));
    assert_eq!(top["surface"]["type"], "plane", "{top}");
    assert!(close3(&top["surface"]["normal"], [0.0, 0.0, 1.0]), "{top}");
    let area = 40.0 * 30.0 - std::f64::consts::PI * 25.0;
    assert!(top["area_mm2"].as_f64().is_some_and(|a| (a - area).abs() < area * 1e-3), "{top}");
    assert!(top["name"].as_str().is_some_and(|n| !n.is_empty()), "persistent face name: {top}");
    assert!(top["name_exact"].is_boolean());

    let hole = &it[1];
    assert_eq!(hole["surface"]["type"], "cylinder", "{hole}");
    assert!(close(&hole["surface"]["radius"], 5.0), "{hole}");
    assert!(hole["surface"]["axis"].as_array().is_some_and(|a| a[2].as_f64().is_some_and(|z| z.abs() > 0.999)), "{hole}");
    assert!(close(&hole["diameter"], 10.0), "{hole}");

    let line = &it[2];
    assert_eq!(line["kind"], "edge");
    assert_eq!(line["curve"], "line", "{line}");
    assert!(close(&line["length_mm"], 40.0), "{line}");
    let ends: Vec<f64> = [&line["start"], &line["end"]].iter().map(|p| p[0].as_f64().unwrap()).collect();
    assert!(ends.iter().any(|x| x.abs() < 1e-9) && ends.iter().any(|x| (x - 40.0).abs() < 1e-9), "{line}");
    assert!(line["name"].as_str().is_some_and(|n| n.contains('|')), "edge names join their faces' names: {line}");

    let rim = &it[3];
    assert_eq!(rim["curve"], "circle", "{rim}");
    assert!(close(&rim["radius"], 5.0), "{rim}");
    assert!(close3(&rim["center"], [20.0, 15.0, 20.0]), "{rim}");

    let b = &it[4];
    assert_eq!(b["kind"], "body");
    assert_eq!(b["name"], json!(body));
    let v = 40.0 * 30.0 * 20.0 - std::f64::consts::PI * 25.0 * 20.0;
    assert!(b["volume_mm3"].as_f64().is_some_and(|x| (x - v).abs() < v * 1e-2), "{b}");
    assert!(b["feature"]["name"].is_string(), "{b}");
    assert_eq!(b["visible"], true);
    assert!(b.get("component").is_none(), "no component outside an assembly");

    // Read-only: nothing changed.
    assert_eq!(s.revision, revision);
    assert_eq!(s.undo.len(), undo);
    assert_eq!(s.is_dirty(), dirty);
    assert!(std::sync::Arc::ptr_eq(&s.doc, &doc));
    assert_eq!(s.selection.len(), 5);

    // Without detail: identity only.
    let r = run(&mut s, "selection.get", json!({"detail": false}));
    let first = r["items"][0].as_object().unwrap();
    assert!(first.contains_key("index") && first.contains_key("point") && !first.contains_key("surface"), "{r}");
    // The items go straight back into select.set.
    let items: Vec<Value> = r["items"].as_array().unwrap().clone();
    run(&mut s, "select.clear", json!({}));
    run(&mut s, "select.set", json!({ "items": items }));
    assert_eq!(s.selection.len(), 5);
}

#[test]
fn sketch_entities_name_their_sketch() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 7}));
    let curve = r["created"]["curves"][0].as_str().unwrap().to_string();
    run(&mut s, "select.set", json!({"items": [{"type": "sketch_curve", "id": curve}]}));
    let r = run(&mut s, "selection.get", json!({}));
    let it = &r["items"][0];
    assert_eq!(it["kind"], "sketch_curve");
    assert_eq!(it["entity"], "circle", "{it}");
    assert!(close(&it["radius"], 7.0), "{it}");
    assert_eq!(it["sketch"]["id"], json!(s.active_sketch.unwrap()), "{it}");
    assert!(it["sketch"]["name"].is_string());
}

#[test]
fn assembly_bodies_report_their_component_and_placement() {
    let mut s = Session::default();
    let created = run(&mut s, "component.create", json!({"name": "Part"}));
    let component = created["component"].as_u64().unwrap();
    let first = created["occurrence"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length": 2, "width": 3, "height": 4, "body_name": "Block", "operation": "new"}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence": first}))["occurrence"].as_u64().unwrap();
    let name = s.occurrence_placement(&[second]).unwrap().bodies[0].world.clone();
    run(&mut s, "select.set", json!({"items": [{"type": "body", "name": name}, {"type": "occurrence", "path": [second]}]}));
    let r = run(&mut s, "selection.get", json!({}));
    let b = &r["items"][0];
    assert_eq!(b["component"]["id"], component, "{b}");
    assert_eq!(b["component"]["name"], "Part", "{b}");
    assert_eq!(b["occurrence_path"], json!([second]), "{b}");
    assert_eq!(b["source_body"], "Block", "{b}");
    let o = &r["items"][1];
    assert_eq!(o["kind"], "occurrence");
    assert_eq!(o["component"]["id"], component, "{o}");
    assert_eq!(o["bodies"], json!([name]), "{o}");
}

#[test]
fn hostile_params_and_stale_selections_never_fail_badly() {
    let mut s = Session::default();
    let body = part(&mut s);
    // Items that point at nothing still come back, by identity.
    let stale = json!([
        {"type": "face", "body": body, "index": 999_999, "point": [1e300, -1e300, 0]},
        {"type": "edge", "body": "nope", "index": 0, "point": [0, 0, 0]},
        {"type": "edge", "body": body, "index": usize::MAX, "point": [0, 0, 0]},
        {"type": "vertex", "body": body, "point": [0, 0, 0]},
        {"type": "feature", "id": u64::MAX},
        {"type": "sketch_curve", "id": "zz"},
        {"type": "sketch_point", "id": "99999999999999999999999:p"},
        {"type": "sketch_constraint", "id": ""},
        {"type": "profile", "sketch": 12345, "index": 3},
        {"type": "plane", "name": "XY"},
        {"type": "axis", "name": "Q"},
    ]);
    run(&mut s, "select.set", json!({ "items": stale }));
    let r = run(&mut s, "selection.get", json!({}));
    assert_eq!(r["count"], 11, "{r}");
    for v in [json!(1), json!("yes"), json!([true]), json!({"a": 1})] {
        let e = s.execute("selection.get", &json!({ "detail": v })).unwrap_err();
        assert!(matches!(e, EngineError::BadParams { .. }), "{e}");
    }
    for v in [json!(null), json!([]), json!("x"), json!(f64::MAX), json!({"detail": null})] {
        assert!(s.execute("selection.get", &v).is_ok(), "{v}");
    }
}
