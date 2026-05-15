use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
union Bad {
    a: u32,
    b: f32,
}

fn main() {}
