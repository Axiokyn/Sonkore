//! The play queue: the tracks, the current one, the
//! shuffle order and the repeat mode. Pure logic; the player plays it.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    /// playback/repeat (0 off, 1 all, 2 one).
    pub fn from_index(i: i64) -> Self {
        match i {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        }
    }
    pub fn index(self) -> i64 {
        self as i64
    }
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    paths: Vec<String>,
    /// The play order: indices into `paths` (shuffled, or 0..n).
    order: Vec<usize>,
    current: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl Queue {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Queue { rng: seed | 1, ..Default::default() }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }
    pub fn len(&self) -> usize {
        self.paths.len()
    }
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
    pub fn current(&self) -> Option<usize> {
        self.current
    }
    pub fn current_path(&self) -> Option<&str> {
        self.current.and_then(|i| self.paths.get(i)).map(String::as_str)
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// Replaces the queue, `start` current.
    pub fn set(&mut self, paths: Vec<String>, start: usize) {
        self.current = if paths.is_empty() { None } else { Some(start.min(paths.len() - 1)) };
        self.paths = paths;
        self.rebuild_order();
    }

    /// Adds at the end; true when the queue was empty (the caller plays it).
    pub fn enqueue(&mut self, paths: Vec<String>) -> bool {
        let was_empty = self.paths.is_empty();
        let first = self.paths.len();
        self.paths.extend(paths);
        if was_empty {
            self.rebuild_order();
        } else {
            // Shuffled, each new track goes somewhere after the current one:
            // the order so far (what played, what "previous" goes back to)
            // stays as it is.
            let after = self.order_position().map_or(0, |p| p + 1);
            for index in first..self.paths.len() {
                let at = if self.shuffle {
                    after + self.next_random(self.order.len() - after + 1)
                } else {
                    self.order.len()
                };
                self.order.insert(at, index);
            }
        }
        was_empty && !self.paths.is_empty()
    }

    pub fn set_current(&mut self, index: Option<usize>) {
        self.current = index.filter(|&i| i < self.paths.len());
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.order.clear();
        self.current = None;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on != self.shuffle {
            self.shuffle = on;
            self.rebuild_order();
        }
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    fn next_random(&mut self, below: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % below as u64) as usize
    }

    /// 0..n, shuffled when shuffle is on with the current track first.
    fn rebuild_order(&mut self) {
        self.order = (0..self.paths.len()).collect();
        if !self.shuffle || self.order.is_empty() {
            return;
        }
        for i in (1..self.order.len()).rev() {
            let j = self.next_random(i + 1);
            self.order.swap(i, j);
        }
        let current = self.current.unwrap_or(0);
        if let Some(at) = self.order.iter().position(|&i| i == current) {
            self.order.swap(0, at);
        }
    }

    fn order_position(&self) -> Option<usize> {
        let current = self.current?;
        self.order.iter().position(|&i| i == current)
    }

    /// The track that follows when the current one ends on its own.
    pub fn upcoming(&self) -> Option<usize> {
        let current = self.current?;
        if self.paths.is_empty() {
            return None;
        }
        if self.repeat == Repeat::One {
            return Some(current);
        }
        let mut pos = self.order_position()? + 1;
        if pos >= self.order.len() {
            if self.repeat == Repeat::Off {
                return None;
            }
            pos = 0;
        }
        self.order.get(pos).copied()
    }

    /// The candidates for "next", in order (for skipping unreadable files):
    /// none past the end when it ended on its own and nothing repeats.
    pub fn next_candidates(&self, user: bool) -> Vec<usize> {
        let mut out = Vec::new();
        let n = self.order.len();
        let mut pos = self.order_position().map_or(usize::MAX, |p| p);
        for _ in 0..n {
            pos = pos.wrapping_add(1);
            if pos >= n {
                if self.repeat == Repeat::Off && !user {
                    break;
                }
                pos = 0;
            }
            out.push(self.order[pos]);
        }
        out
    }

    /// The track "previous" goes to (wrapping only under repeat all).
    pub fn previous_index(&self) -> Option<usize> {
        if self.paths.is_empty() {
            return None;
        }
        let pos = self.order_position().unwrap_or(0);
        let pos = if pos == 0 { if self.repeat == Repeat::All { self.order.len() - 1 } else { 0 } } else { pos - 1 };
        self.order.get(pos).copied()
    }

    /// Removes entries; the current one keeps playing and "next" plays what
    /// followed it.
    pub fn remove(&mut self, indices: &[usize]) -> bool {
        let size = self.paths.len();
        let mut removed = vec![false; size];
        let mut any = false;
        for &i in indices.iter().filter(|&&i| i < size) {
            removed[i] = true;
            any = true;
        }
        if !any {
            return false;
        }
        // The current entry, or the one before it in the play order when it
        // goes: "next" then plays what followed it.
        let current =
            self.order_position().and_then(|pos| self.order[..=pos].iter().rev().find(|&&i| !removed[i]).copied());
        let mut new_index = vec![usize::MAX; size];
        let mut kept = 0;
        for (i, slot) in new_index.iter_mut().enumerate() {
            if !removed[i] {
                *slot = kept;
                kept += 1;
            }
        }
        let mut i = 0;
        self.paths.retain(|_| {
            i += 1;
            !removed[i - 1]
        });
        // The play order stays, without the entries removed.
        self.order.retain(|&i| !removed[i]);
        for i in &mut self.order {
            *i = new_index[*i];
        }
        self.current = current.map(|c| new_index[c]);
        true
    }

    /// Moves `indices` before `target` (len = the end).
    pub fn move_entries(&mut self, indices: &[usize], target: usize) -> bool {
        let size = self.paths.len();
        let mut moved: Vec<usize> = indices.iter().copied().filter(|&i| i < size).collect();
        moved.sort_unstable();
        moved.dedup();
        if moved.is_empty() {
            return false;
        }
        let target = target.min(size);
        let mut is_moved = vec![false; size];
        for &i in &moved {
            is_moved[i] = true;
        }
        let mut order = Vec::with_capacity(size);
        for i in 0..size {
            if i == target {
                order.extend_from_slice(&moved);
            }
            if !is_moved[i] {
                order.push(i);
            }
        }
        if target == size {
            order.extend_from_slice(&moved);
        }
        self.apply_order(&order)
    }

    pub fn reverse(&mut self) -> bool {
        let order: Vec<usize> = (0..self.paths.len()).rev().collect();
        self.apply_order(&order)
    }

    /// Puts the entries in `order` (old indices); the current track and the
    /// shuffle order follow their entries.
    pub fn apply_order(&mut self, order: &[usize]) -> bool {
        let size = self.paths.len();
        if order.len() != size {
            return false;
        }
        let mut new_index = vec![0; size];
        let mut paths = Vec::with_capacity(size);
        for (n, &old) in order.iter().enumerate() {
            new_index[old] = n;
            paths.push(self.paths[old].clone());
        }
        if paths == self.paths {
            return false;
        }
        self.paths = paths;
        self.current = self.current.map(|c| new_index[c]);
        for i in &mut self.order {
            *i = new_index[*i];
        }
        true
    }

    /// Drops what is inside `folder`; true when the current track was there.
    pub fn forget_folder(&mut self, folder: &str) -> bool {
        let prefix = if folder.ends_with('/') { folder.to_owned() } else { format!("{folder}/") };
        let inside = |p: &String| p.starts_with(&prefix);
        if self.current_path().is_some_and(|p| p.starts_with(&prefix)) {
            self.paths.retain(|p| !inside(p));
            self.current = None;
            self.rebuild_order();
            return true;
        }
        let indices: Vec<usize> = self.paths.iter().enumerate().filter(|(_, p)| inside(p)).map(|(i, _)| i).collect();
        self.remove(&indices);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: usize) -> Queue {
        let mut q = Queue::new();
        q.set((0..n).map(|i| format!("/m/{i}.flac")).collect(), 0);
        q
    }

    #[test]
    fn upcoming_and_repeat() {
        let mut q = q(3);
        q.set_current(Some(2));
        assert_eq!(q.upcoming(), None);
        q.set_repeat(Repeat::All);
        assert_eq!(q.upcoming(), Some(0));
        q.set_repeat(Repeat::One);
        assert_eq!(q.upcoming(), Some(2));
        q.set_repeat(Repeat::Off);
        assert_eq!(q.next_candidates(false), Vec::<usize>::new());
        assert_eq!(q.next_candidates(true), vec![0, 1, 2]);
    }

    #[test]
    fn previous_wraps_only_under_repeat_all() {
        let mut q = q(3);
        assert_eq!(q.previous_index(), Some(0));
        q.set_repeat(Repeat::All);
        assert_eq!(q.previous_index(), Some(2));
    }

    #[test]
    fn shuffle_starts_with_the_current_track() {
        let mut q = q(50);
        q.set_current(Some(17));
        q.set_shuffle(true);
        assert_eq!(q.order()[0], 17);
        let mut sorted = q.order().to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn remove_and_move_keep_the_current_track() {
        let mut q = q(5);
        q.set_current(Some(3));
        q.remove(&[0, 1]);
        assert_eq!(q.current_path(), Some("/m/3.flac"));
        q.move_entries(&[0], 3); // 2 moves to the end
        assert_eq!(q.paths(), ["/m/3.flac", "/m/4.flac", "/m/2.flac"]);
        assert_eq!(q.current_path(), Some("/m/3.flac"));
        q.reverse();
        assert_eq!(q.current(), Some(2));
    }

    #[test]
    fn shuffle_keeps_its_order_when_tracks_come_and_go() {
        // Adding or removing a track reshuffled everything: tracks already
        // played came back and "previous" went elsewhere.
        let mut q = q(50);
        q.set_shuffle(true);
        for _ in 0..10 {
            let next = q.upcoming().unwrap();
            q.set_current(Some(next));
        }
        let played: Vec<usize> = q.order()[..=10].to_vec();
        assert_eq!(q.current(), Some(played[10]));
        q.enqueue(vec!["/m/new-a.flac".into(), "/m/new-b.flac".into()]);
        assert_eq!(&q.order()[..=10], &played[..], "the order so far stays");
        assert_eq!(q.order().len(), 52);
        for new in [50, 51] {
            assert!(q.order().iter().position(|&i| i == new).unwrap() > 10, "new tracks come later");
        }
        assert_eq!(q.previous_index(), Some(played[9]));
        // Removing other tracks keeps everyone's place.
        let names = |q: &Queue| q.order().iter().map(|&i| q.paths()[i].clone()).collect::<Vec<_>>();
        let gone = [q.order()[3], q.order()[30]];
        let mut want = names(&q);
        want.retain(|p| *p != q.paths()[gone[0]] && *p != q.paths()[gone[1]]);
        let current = q.current_path().unwrap().to_owned();
        q.remove(&gone);
        assert_eq!(names(&q), want);
        assert_eq!(q.current_path(), Some(current.as_str()));
        // Removing the current one: "next" is what followed it.
        let pos = q.order().iter().position(|&i| Some(i) == q.current()).unwrap();
        let following = q.paths()[q.order()[pos + 1]].clone();
        q.remove(&[q.current().unwrap()]);
        assert_eq!(q.upcoming().map(|i| q.paths()[i].clone()), Some(following));
    }

    #[test]
    fn removing_the_current_points_before_it() {
        let mut q = q(4);
        q.set_current(Some(2));
        q.remove(&[2]);
        assert_eq!(q.current(), Some(1)); // "next" plays what followed it
    }
}
