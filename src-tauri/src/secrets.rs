//! Admin API key en el llavero del sistema (crate `keyring`).
//!
//! La clave solo se guarda aquí: nunca en archivos, ajustes ni frontend.
//! El frontend puede enviarla una vez para guardarla, pero ningún comando la
//! devuelve: solo se informa de si existe.

use std::fmt;

use keyring::v1::{Entry, Error};

const SERVICE: &str = "monitor-uso-claude";
const ACCOUNT: &str = "anthropic-admin-key";

/// Admin key en memoria. Su `Debug` no muestra el valor y no implementa
/// `Display`, para que no pueda acabar en un log por descuido.
pub struct AdminKey(String);

impl AdminKey {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AdminKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdminKey(<oculta>)")
    }
}

/// Error del llavero, con un mensaje apto para mostrar (nunca incluye la clave).
#[derive(Debug)]
pub struct SecretError(pub String);

fn entry() -> Result<Entry, SecretError> {
    Entry::new(SERVICE, ACCOUNT)
        .map_err(|_| SecretError("no se puede acceder al llavero del sistema".into()))
}

/// Comprueba el formato antes de guardar, para avisar de claves pegadas mal.
/// No valida contra la API.
pub fn looks_like_admin_key(key: &str) -> bool {
    let key = key.trim();
    key.starts_with("sk-ant-admin") && key.len() > 20 && !key.chars().any(char::is_whitespace)
}

pub fn save_admin_key(key: &str) -> Result<(), SecretError> {
    let key = key.trim();
    if !looks_like_admin_key(key) {
        return Err(SecretError(
            "no parece una Admin key (empiezan por sk-ant-admin)".into(),
        ));
    }
    entry()?
        .set_password(key)
        .map_err(|_| SecretError("no se pudo guardar la clave en el llavero".into()))
}

pub fn load_admin_key() -> Result<Option<AdminKey>, SecretError> {
    match entry()?.get_password() {
        Ok(key) => Ok(Some(AdminKey(key))),
        Err(Error::NoEntry) => Ok(None),
        Err(_) => Err(SecretError("no se pudo leer el llavero del sistema".into())),
    }
}

pub fn delete_admin_key() -> Result<(), SecretError> {
    match entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(_) => Err(SecretError("no se pudo borrar la clave del llavero".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_key_format() {
        assert!(looks_like_admin_key("sk-ant-admin01-abcdefghijklmnop"));
        assert!(looks_like_admin_key("  sk-ant-admin01-abcdefghijklmnop \n"));
        assert!(!looks_like_admin_key("sk-ant-api03-abcdefghijklmnop"));
        assert!(!looks_like_admin_key("sk-ant-admin01"));
        assert!(!looks_like_admin_key("sk-ant-admin01-abc def ghijklmnop"));
        assert!(!looks_like_admin_key(""));
    }

    #[test]
    fn debug_hides_the_key() {
        let key = AdminKey("<ADMIN_KEY>".into());
        assert!(!format!("{key:?}").contains("<ADMIN_KEY>"));
    }
}
