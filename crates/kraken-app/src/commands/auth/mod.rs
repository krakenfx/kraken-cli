//! `kraken auth`: one file per subcommand. The key store, the precedence and
//! the sign-in protocol live in `kraken-auth`; this module only asks and
//! renders.
mod active_key;
mod login;
mod reset;
mod secret;
mod set;
mod show;
mod switch;
mod test;

pub(crate) use active_key::offer_new_key;
use kraken_contract::auth::AuthCommand;
pub(crate) use secret::is_decodable_secret;

delegate_execute!(AuthCommand {
    Login,
    Set,
    Show,
    Switch,
    Test,
    Reset
});
