#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct Cache<T>(KeyedCache<(), T>);

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Cache<T> {
    pub fn new() -> Self {
        Self(KeyedCache::new())
    }

    pub fn get(&self) -> Option<&T> {
        self.0.get().map(|(_, v)| v)
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.0.get_mut().map(|(_, v)| v)
    }

    pub fn get_or_set_ref(
        &mut self,
        matches: impl FnOnce(&T) -> bool,
        value_factory: impl FnOnce() -> T,
    ) -> &mut T {
        self.0
            .get_or_set_ref_indirect(|_, t| matches(t), || ((), value_factory()))
    }

    pub fn try_get_or_set_ref<E>(
        &mut self,
        matches: impl FnOnce(&T) -> bool,
        value_factory: impl FnOnce() -> Result<T, E>,
    ) -> Result<&mut T, E> {
        self.0
            .try_get_or_set_ref_indirect(|_, t| matches(t), || Ok(((), value_factory()?)))
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct KeyedCache<K, T>(Option<(K, T)>);

impl<K, T> KeyedCache<K, T> {
    pub fn new() -> Self {
        Self(None)
    }

    pub fn from_data(key: K, value: T) -> Self {
        Self(Some((key, value)))
    }

    pub fn get(&self) -> Option<(&K, &T)> {
        self.0.as_ref().map(|(k, v)| (k, v))
    }

    pub fn get_mut(&mut self) -> Option<(&mut K, &mut T)> {
        self.0.as_mut().map(|(k, v)| (k, v))
    }

    pub fn get_by(&self, key: &K) -> Option<&T>
    where
        K: Eq,
    {
        self.get_by_indirect(|k| k == key)
    }

    pub fn get_by_indirect(&self, key_checker: impl FnOnce(&K) -> bool) -> Option<&T> {
        match &self.0 {
            Some((k, v)) if key_checker(k) => Some(v),
            _ => None,
        }
    }

    pub fn get_or_set_ref(&mut self, key: &K, value_factory: impl FnOnce() -> T) -> &mut T
    where
        K: Eq + Clone,
    {
        self.try_get_or_set_ref::<()>(key, || Ok(value_factory()))
            .unwrap()
    }

    pub fn get_or_set_ref_indirect(
        &mut self,
        checker: impl FnOnce(&K, &T) -> bool,
        value_factory: impl FnOnce() -> (K, T),
    ) -> &mut T {
        self.try_get_or_set_ref_indirect::<()>(checker, || Ok(value_factory()))
            .unwrap()
    }

    pub fn try_get_or_set_ref<E>(
        &mut self,
        key: &K,
        value_factory: impl FnOnce() -> Result<T, E>,
    ) -> Result<&mut T, E>
    where
        K: Eq + Clone,
    {
        self.try_get_or_set_ref_indirect(|k, _| k == key, || Ok((key.clone(), value_factory()?)))
    }

    pub async fn try_get_or_set_ref_async<E, F>(
        &mut self,
        key: &K,
        value_factory: impl FnOnce() -> F,
    ) -> Result<&mut T, E>
    where
        K: Eq + Clone,
        F: Future<Output = Result<T, E>>,
    {
        self.try_get_or_set_ref_indirect_async(
            |k, _| k == key,
            async move || Ok((key.clone(), value_factory().await?)),
        )
        .await
    }

    pub fn try_get_or_set_ref_indirect<E>(
        &mut self,
        checker: impl FnOnce(&K, &T) -> bool,
        value_factory: impl FnOnce() -> Result<(K, T), E>,
    ) -> Result<&mut T, E> {
        match &self.0 {
            Some((k, t)) if checker(k, t) => {}
            _ => self.0 = Some(value_factory()?),
        }
        Ok(&mut self.0.as_mut().unwrap().1)
    }

    pub async fn try_get_or_set_ref_indirect_async<E, F>(
        &mut self,
        checker: impl FnOnce(&K, &T) -> bool,
        value_factory: impl FnOnce() -> F,
    ) -> Result<&mut T, E>
    where
        F: Future<Output = Result<(K, T), E>>,
    {
        match &self.0 {
            Some((k, t)) if checker(k, t) => {}
            _ => self.0 = Some(value_factory().await?),
        }
        Ok(&mut self.0.as_mut().unwrap().1)
    }
}

impl<K, T> Default for KeyedCache<K, T> {
    fn default() -> Self {
        Self::new()
    }
}
