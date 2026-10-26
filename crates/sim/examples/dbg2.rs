use sim::*;
fn main() {
    let mu = 2e5;
    let (p, v) = kepler::ejection(mu, 6000.0, v2(1.0, 2.5));
    let r0m = p.len(); let alpha = 2.0/r0m - v.len2()/mu; let smu = mu.sqrt(); let vr0 = p.dot(v)/r0m; let dt = 1e6;
    for chi in [100.0, 500.0, 750.0, 1000.0, 5000.0, 37000.0, 74000.0] {
        let z = alpha*chi*chi; let (c,s) = kepler::stumpff(z);
        let f = r0m * vr0 / smu * chi * chi * c + (1.0 - alpha * r0m) * chi * chi * chi * s + r0m * chi - smu * dt;
        println!("chi {chi} z {z:.1} f {f:e}");
    }
}
