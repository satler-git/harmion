// Pure logic tests modeling a minimal pool to validate common semantics (capacity, acquire/release).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dummy(u32);

#[derive(Debug)]
struct MiniPool {
    items: Vec<Dummy>,
    cap: usize,
}
impl MiniPool {
    fn with_capacity(cap: usize) -> Self { Self { items: Vec::new(), cap } }
    fn capacity(&self) -> usize { self.cap }
    fn len(&self) -> usize { self.items.len() }
    fn is_empty(&self) -> bool { self.items.is_empty() }
    fn acquire(&mut self, id: u32) -> Result<(), &'static str> {
        if self.items.len() >= self.cap { return Err("pool full"); }
        self.items.push(Dummy(id));
        Ok(())
    }
    fn release(&mut self, id: u32) -> bool {
        if let Some(i) = self.items.iter().position(|d| d.0 == id) {
            self.items.remove(i);
            true
        } else { false }
    }
}

#[test]
fn new_pool_is_empty_and_has_capacity() {
    let p = MiniPool::with_capacity(2);
    assert!(p.is_empty());
    assert_eq!(p.capacity(), 2);
}

#[test]
fn acquire_within_capacity_succeeds_and_increments_length() {
    let mut p = MiniPool::with_capacity(2);
    assert!(p.acquire(1).is_ok());
    assert!(p.acquire(2).is_ok());
    assert_eq!(p.len(), 2);
}

#[test]
fn acquire_beyond_capacity_fails_with_specific_error() {
    let mut p = MiniPool::with_capacity(1);
    p.acquire(9).unwrap();
    assert_eq!(p.acquire(10).unwrap_err(), "pool full");
}

#[test]
fn release_removes_existing_and_reports_missing() {
    let mut p = MiniPool::with_capacity(2);
    p.acquire(7).unwrap();
    assert!(p.release(7));
    assert!(!p.release(8));
    assert!(p.is_empty());
}