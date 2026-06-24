use crate::types::Qty;

/// FIFO queue of order-slot indices at one price. Intrusive doubly-linked list
/// through Order::next/prev so cancel-from-middle is O(1) without scanning.
#[derive(Debug, Clone, Default)]
pub struct PriceLevel {
    pub head: Option<usize>,
    pub tail: Option<usize>,
    pub total_qty: Qty,
    pub count: usize,
}

impl PriceLevel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    /// Append slot to tail (time priority: new orders go to back).
    pub fn push_back(&mut self, slot: usize, qty: Qty) {
        self.total_qty += qty;
        self.count += 1;
        // caller must wire Order::prev/next
        if self.tail.is_none() {
            self.head = Some(slot);
            self.tail = Some(slot);
        } else {
            self.tail = Some(slot);
        }
    }

    /// Remove head slot (best time priority = first to fill).
    pub fn pop_front(&mut self, qty: Qty) -> Option<usize> {
        let slot = self.head?;
        self.total_qty = self.total_qty.saturating_sub(qty);
        Some(slot)
    }

    /// Remove an arbitrary slot (cancel). Caller must fix linked-list pointers.
    pub fn remove(&mut self, qty: Qty) {
        self.total_qty = self.total_qty.saturating_sub(qty);
        self.count = self.count.saturating_sub(1);
    }
}
