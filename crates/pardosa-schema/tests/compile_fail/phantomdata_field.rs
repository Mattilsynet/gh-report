use pardosa_schema::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    marker: core::marker::PhantomData<u32>,
}

fn main() {}
