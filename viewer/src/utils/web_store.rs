use eframe::Result;
use idb::{Database, DatabaseEvent, ObjectStoreParams};
use wasm_bindgen::JsValue;
use web_sys::js_sys;

pub struct WebStore {
    database: Database,
}

impl WebStore {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn open() -> Result<Self, idb::Error> {
        let factory = idb::Factory::new()?;
        let mut database = factory.open("default", Some(1))?;
        database.on_upgrade_needed(|evt| {
            let database = evt.database().unwrap();
            let mut store_params = ObjectStoreParams::new();
            store_params.auto_increment(true);
            store_params.key_path(Some(idb::KeyPath::new_single("id")));
            database.create_object_store("store", store_params).unwrap();
        });
        let database = database.await?;
        Ok(Self { database })
    }

    pub async fn set(&self, value: JsValue) -> Result<u32, idb::Error> {
        let tx = self
            .database
            .transaction(&["store"], idb::TransactionMode::ReadWrite)?;
        let store = tx.object_store("store")?;
        let result = store.put(&value, None)?.await?;
        let id = js_sys::Reflect::get(&result, &JsValue::from_str("id"))
            .map_err(|e| idb::Error::KeyPathNotFound(e))?
            .as_f64()
            .ok_or_else(|| idb::Error::UnexpectedJsType("Number", JsValue::null()))?
            as u32;
        tx.commit()?.await?;
        Ok(id)
    }

    pub async fn get(&self, key: u32) -> Result<Option<JsValue>, idb::Error> {
        let tx = self
            .database
            .transaction(&["store"], idb::TransactionMode::ReadOnly)?;
        let store = tx.object_store("store")?;
        let value = store.get(JsValue::from_f64(key.into()))?.await?;
        tx.await?;
        Ok(value)
    }
}
