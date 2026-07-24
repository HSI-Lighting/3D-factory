//! Demo: build a perturbed quad, constrain it into a rectangle, solve, and
//! round-trip through the .rsmp format.
//!
//!   cargo run -p cad_param --example solve_demo

use cad_param::{read_rsmp, solve, write_rsmp, Constraint, Sketch};

fn main() {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.4);
    let p2 = s.add_point(9.7, 5.2);
    let p3 = s.add_point(0.3, 4.8);
    let l0 = s.add_line(p0, p1);
    let l1 = s.add_line(p1, p2);
    let l2 = s.add_line(p2, p3);
    let l3 = s.add_line(p3, p0);
    s.add(Constraint::Fixed { p: p0, x: 0.0, y: 0.0 });
    s.add(Constraint::Horizontal { line: l0 });
    s.add(Constraint::Vertical { line: l1 });
    s.add(Constraint::Horizontal { line: l2 });
    s.add(Constraint::Vertical { line: l3 });
    s.add(Constraint::Distance { p: p0, q: p1, d: 10.0 });
    s.add(Constraint::Distance { p: p1, q: p2, d: 5.0 });

    println!("before: {:?}", s.points);
    println!("dof = {}  ({} points, {} residuals)",
             s.dof(), s.points.len(), s.residual_dim());

    let rep = solve(&mut s);
    println!("\nsolve: converged={} iters={} rms={:.2e} dof={}",
             rep.converged, rep.iterations, rep.residual, rep.dof);
    for (i, p) in s.points.iter().enumerate() {
        println!("  p{i} = ({:.4}, {:.4})", p.x, p.y);
    }

    // .rsmp round-trip
    let text = write_rsmp(&s);
    let back = read_rsmp(&text).expect("rsmp round-trip");
    println!("\n.rsmp round-trip: {} pts, {} lines, {} constraints",
             back.points.len(), back.lines.len(), back.constraints.len());
}
