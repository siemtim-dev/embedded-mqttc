use heapless::Vec;


pub trait RemoveAndExt<T> {

    fn remove_and<'a, F>(&'a mut self, remove_where: F) -> impl Iterator<Item = T>
        where F: Fn(&T) -> bool;

}

pub struct RemoveAndVecIterator<'a, T, F: Fn(&T) -> bool, const N: usize> {
    base: &'a mut Vec<T, N>,
    remove_where: F,
    i: usize
}

impl <'a, T, F: Fn(&T) -> bool, const N: usize> Iterator for RemoveAndVecIterator<'a, T, F, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        while self.i < self.base.len() {
            if (self.remove_where)(&self.base[self.i]) {

                return Some(self.base.remove(self.i));
            } else {
                self.i += 1;
            }
        }

        None
    }
}

impl <T, const N: usize> RemoveAndExt<T> for Vec<T, N> {
    fn remove_and<'a, F>(&'a mut self, remove_where: F) -> impl Iterator<Item = T>
        where F: Fn(&T) -> bool {
            RemoveAndVecIterator {
                base: self,
                remove_where,
                i: 0
            }
    }
}

#[cfg(test)]
mod tests {

    use super::RemoveAndExt;

    #[test]
    fn test_remove_numbers() {
        let mut numbers = heapless::Vec::<usize, 8>::new();
        for i in 0..8 {
            numbers.push(i);
        }

        let mut removed = heapless::Vec::<usize, 8>::new();
        numbers.remove_and( |el| el % 2 == 0 )
            .for_each(|el| {
                removed.push(el);
            });

        assert_eq!(&numbers[..], &[1, 3, 5, 7]);
        assert_eq!(&removed[..], &[0, 2, 4, 6]);

    }

}