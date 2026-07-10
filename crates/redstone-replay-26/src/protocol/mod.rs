pub(crate) mod buf;
pub(crate) mod chunk;
pub(crate) mod packets;
pub(crate) mod registry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PacketState {
    Login,
    Configuration,
    Play,
}
