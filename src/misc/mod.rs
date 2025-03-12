
use heapless::Vec;

pub trait AsVec<T, const N: usize> {
    fn as_vec(self) -> Vec<T, N>;
}

impl <T, const N: usize> AsVec<T, N> for Option<T> {
    fn as_vec(self) -> Vec<T, N> {
        let mut v = Vec::new();
        if let Some(el) = self {
            unsafe{
                v.push_unchecked(el);
            }
        }

        v
    }
}

impl <T, const N: usize, const M: usize> AsVec<T, N> for Vec<T, M> {
    fn as_vec(self) -> Vec<T, N> {
        let mut v = Vec::new();

        for el in self {
            let result = v.push(el);
            if let Err(_) = result {
                panic!("vec capacity too small");
            }
        }

        v
    }
}
