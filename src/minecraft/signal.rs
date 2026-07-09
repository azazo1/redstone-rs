use crate::core::{
    BlockEvent, BlockKind, BlockState, ComparatorMode, Direction, Position, ScheduledTick,
    TickPriority, World, WorldError,
};

use super::piston;

pub(crate) fn handle_neighbor_changed(
    world: &mut World,
    position: Position,
    _changed_block: BlockKind,
    source: Position,
) {
    let state = world.state(position);
    match state.kind {
        BlockKind::RedstoneWire => refresh_wire(world, position),
        BlockKind::RedstoneTorch => schedule_torch_update(world, position, state),
        BlockKind::Repeater => schedule_repeater_update(world, position, state),
        BlockKind::Comparator => schedule_comparator_update(world, position, state),
        BlockKind::Observer => {
            if source == position.offset(state.facing()) && !state.powered() && !world.has_scheduled_tick(position, BlockKind::Observer) {
                world.schedule_tick(position, BlockKind::Observer, 2, TickPriority::Normal);
            }
        }
        BlockKind::Piston | BlockKind::StickyPiston => piston::check_piston(world, position, state),
        BlockKind::Lamp => update_lamp_from_neighbor(world, position, state),
        BlockKind::CopperBulb => update_copper_bulb(world, position, state),
        BlockKind::Door | BlockKind::Trapdoor | BlockKind::FenceGate => update_openable(world, position, state),
        BlockKind::NoteBlock | BlockKind::Dropper | BlockKind::Dispenser | BlockKind::Crafter | BlockKind::Tnt => {
            update_pulse_receiver(world, position, state)
        }
        _ => {}
    }
}

pub(crate) fn handle_scheduled_tick(world: &mut World, tick: ScheduledTick) {
    if world.state(tick.position).kind != tick.block {
        return;
    }
    match tick.block {
        BlockKind::RedstoneTorch => update_torch(world, tick.position),
        BlockKind::Button => {
            let state = world.state(tick.position);
            set_powered(world, tick.position, state, false, "button_timeout");
            notify_attached_conductors(world, tick.position, BlockKind::Button);
        }
        BlockKind::Repeater => update_repeater(world, tick.position),
        BlockKind::Comparator => update_comparator(world, tick.position),
        BlockKind::Observer => update_observer(world, tick.position),
        BlockKind::Lamp => update_lamp_tick(world, tick.position),
        BlockKind::Target => reset_target(world, tick.position),
        _ => {}
    }
}

pub(crate) fn handle_block_event(world: &mut World, event: BlockEvent) -> bool {
    if event.action == 1 && world.state(event.position).kind == BlockKind::Tnt {
        let _ = world.set_state(event.position, BlockState::new(BlockKind::Air), "tnt_trigger".to_owned());
        return true;
    }
    false
}

pub(crate) fn signal_from(world: &World, source: Position, toward: Direction) -> u8 {
    let state = world.state(source);
    match state.kind {
        BlockKind::Solid
        | BlockKind::Immovable
        | BlockKind::Container
        | BlockKind::Piston
        | BlockKind::StickyPiston
        | BlockKind::SlimeBlock
        | BlockKind::HoneyBlock => conductor_signal(world, source),
        _ => direct_signal_from(state, toward),
    }
}

fn direct_signal_from(state: BlockState, toward: Direction) -> u8 {
    match state.kind {
        BlockKind::RedstoneBlock => 15,
        BlockKind::DaylightDetector | BlockKind::Target => state.power(),
        BlockKind::RedstoneWire => {
            if toward == Direction::Up { 0 } else { state.power() }
        }
        BlockKind::RedstoneTorch => {
            if state.powered() && toward != Direction::Down {
                15
            } else {
                0
            }
        }
        BlockKind::Lever | BlockKind::Button | BlockKind::PressurePlate => u8::from(state.powered()) * 15,
        BlockKind::Repeater => {
            if state.powered() && toward == state.facing() {
                15
            } else {
                0
            }
        }
        BlockKind::Comparator => {
            if state.powered() && toward == state.facing() {
                state.power()
            } else {
                0
            }
        }
        BlockKind::Observer if state.powered() && toward == state.facing().opposite() => 15,
        _ => 0,
    }
}

fn conductor_signal(world: &World, position: Position) -> u8 {
    Direction::ALL
        .into_iter()
        .map(|direction| direct_conductor_signal_from(world.state(position.offset(direction)), direction.opposite()))
        .max()
        .unwrap_or(0)
}

fn direct_conductor_signal_from(state: BlockState, toward: Direction) -> u8 {
    if state.kind == BlockKind::RedstoneWire && toward != Direction::Down {
        0
    } else {
        direct_signal_from(state, toward)
    }
}

pub(crate) fn received_signal(world: &World, position: Position) -> u8 {
    Direction::ALL
        .into_iter()
        .map(|direction| {
            let source = position.offset(direction);
            signal_from(world, source, direction.opposite())
        })
        .max()
        .unwrap_or(0)
}

pub(crate) fn input_signal(world: &World, position: Position, facing: Direction) -> u8 {
    let rear = position.offset(facing.opposite());
    signal_from(world, rear, facing)
        .max(if world.state(rear).kind.is_conductor() { received_signal(world, rear) } else { 0 })
}

fn side_signal(world: &World, position: Position, facing: Direction) -> u8 {
    Direction::ALL
        .into_iter()
        .filter(|direction| *direction != facing && *direction != facing.opposite())
        .map(|direction| signal_from(world, position.offset(direction), direction.opposite()))
        .max()
        .unwrap_or(0)
}

fn refresh_wire(world: &mut World, position: Position) {
    let state = world.state(position);
    let direct = received_non_wire_signal(world, position);
    let connected_wires = connected_wire_positions(world, position);
    let incoming_wire = connected_wires
        .iter()
        .map(|neighbor| world.state(*neighbor).power().saturating_sub(1))
        .max()
        .unwrap_or(0);
    let power = direct.max(incoming_wire).min(15);
    let next = state.with_power(power).with_powered(power > 0);
    if next != state {
        let _ = world.set_state(position, next, "wire_power");
        for neighbor in connected_wires {
            world.neighbor_changed(neighbor, BlockKind::RedstoneWire);
        }
    }
}

fn connected_wire_positions(world: &World, position: Position) -> Vec<Position> {
    let mut connections = Vec::with_capacity(Direction::HORIZONTAL.len());
    for direction in Direction::HORIZONTAL {
        let adjacent = position.offset(direction);
        let adjacent_state = world.state(adjacent);
        if adjacent_state.kind == BlockKind::RedstoneWire {
            connections.push(adjacent);
            continue;
        }
        if adjacent_state.kind.is_conductor() {
            let upward = adjacent.offset(Direction::Up);
            if world.state(upward).kind == BlockKind::RedstoneWire {
                connections.push(upward);
            }
        } else {
            let downward = adjacent.offset(Direction::Down);
            if world.state(downward).kind == BlockKind::RedstoneWire {
                connections.push(downward);
            }
        }
    }
    connections
}

fn received_non_wire_signal(world: &World, position: Position) -> u8 {
    Direction::ALL
        .into_iter()
        .filter_map(|direction| {
            let source = position.offset(direction);
            (world.state(source).kind != BlockKind::RedstoneWire)
                .then_some(signal_from(world, source, direction.opposite()))
        })
        .max()
        .unwrap_or(0)
}

fn schedule_torch_update(world: &mut World, position: Position, state: BlockState) {
    let desired = received_signal(world, position.offset(Direction::Down)) == 0;
    if desired != state.powered() && !world.has_scheduled_tick(position, BlockKind::RedstoneTorch) {
        world.schedule_tick(position, BlockKind::RedstoneTorch, 2, TickPriority::Normal);
    }
}

fn update_torch(world: &mut World, position: Position) {
    let state = world.state(position);
    let has_neighbor_signal = received_signal(world, position.offset(Direction::Down)) > 0;
    if state.powered() && has_neighbor_signal {
        set_powered(world, position, state, false, "torch_turn_off");
        if world.record_torch_turn_off(position) {
            world.burn_torch(position);
            world.schedule_tick(position, BlockKind::RedstoneTorch, 160, TickPriority::Normal);
        }
    } else if !state.powered() && !has_neighbor_signal && !world.torch_is_burned(position) {
        set_powered(world, position, state, true, "torch_turn_on");
    }
}

fn schedule_repeater_update(world: &mut World, position: Position, state: BlockState) {
    let locked = side_signal(world, position, state.facing()) > 0;
    if locked != state.locked() {
        let _ = world.set_state(position, state.with_locked(locked), "repeater_lock");
    }
    if !locked
        && (input_signal(world, position, state.facing()) > 0) != state.powered()
        && !world.has_scheduled_tick(position, BlockKind::Repeater)
    {
        world.schedule_tick(
            position,
            BlockKind::Repeater,
            state.delay() as u64 * 2,
            diode_priority(world, position, state),
        );
    }
}

fn update_repeater(world: &mut World, position: Position) {
    let state = world.state(position);
    if state.locked() {
        return;
    }
    let powered = input_signal(world, position, state.facing()) > 0;
    let next = state.with_powered(powered).with_power(if powered { 15 } else { 0 });
    if next != state {
        let _ = world.set_state(position, next, "repeater_tick");
    }
}

fn schedule_comparator_update(world: &mut World, position: Position, state: BlockState) {
    let output = comparator_output(world, position, state);
    if output != state.power() && !world.has_scheduled_tick(position, BlockKind::Comparator) {
        world.schedule_tick(position, BlockKind::Comparator, 1, diode_priority(world, position, state));
    }
}

fn diode_priority(world: &World, position: Position, state: BlockState) -> TickPriority {
    let behind = world.state(position.offset(state.facing().opposite()));
    if matches!(behind.kind, BlockKind::Repeater | BlockKind::Comparator)
        && behind.facing() != state.facing().opposite()
    {
        TickPriority::ExtremelyHigh
    } else if state.powered() {
        TickPriority::VeryHigh
    } else {
        TickPriority::High
    }
}

fn update_comparator(world: &mut World, position: Position) {
    let state = world.state(position);
    let power = comparator_output(world, position, state);
    let next = state.with_power(power).with_powered(power > 0);
    if next != state {
        let _ = world.set_state(position, next, "comparator_tick");
    }
}

fn comparator_output(world: &World, position: Position, state: BlockState) -> u8 {
    let rear = position.offset(state.facing().opposite());
    let input = world
        .block_entity(rear)
        .map(|entity| entity.comparator_signal)
        .unwrap_or_else(|| input_signal(world, position, state.facing()));
    let side = side_signal(world, position, state.facing());
    match state.mode() {
        ComparatorMode::Compare if input >= side => input,
        ComparatorMode::Compare => 0,
        ComparatorMode::Subtract => input.saturating_sub(side),
    }
}

fn update_observer(world: &mut World, position: Position) {
    let state = world.state(position);
    let powered = !state.powered();
    let next = state.with_powered(powered).with_power(if powered { 15 } else { 0 });
    if next != state {
        let _ = world.set_state(position, next, "observer_tick");
    }
    if powered {
        world.schedule_tick(position, BlockKind::Observer, 2, TickPriority::Normal);
    }
}

fn update_lamp_from_neighbor(world: &mut World, position: Position, state: BlockState) {
    if received_signal(world, position) > 0 {
        set_powered(world, position, state, true, "lamp_power_on");
    } else if state.powered() && !world.has_scheduled_tick(position, BlockKind::Lamp) {
        world.schedule_tick(position, BlockKind::Lamp, 4, TickPriority::Normal);
    }
}

fn update_lamp_tick(world: &mut World, position: Position) {
    let state = world.state(position);
    if received_signal(world, position) == 0 {
        set_powered(world, position, state, false, "lamp_power_off");
    }
}

fn update_copper_bulb(world: &mut World, position: Position, state: BlockState) {
    let powered = received_signal(world, position) > 0;
    if powered == state.powered() {
        return;
    }
    let next = if powered {
        state.with_powered(true).with_lit(!state.lit())
    } else {
        state.with_powered(false)
    };
    let _ = world.set_state(position, next, "copper_bulb_edge");
}

fn update_openable(world: &mut World, position: Position, state: BlockState) {
    let powered = received_signal(world, position) > 0;
    if powered != state.powered() {
        set_powered(world, position, state, powered, "openable_power");
    }
}

fn update_pulse_receiver(world: &mut World, position: Position, state: BlockState) {
    let powered = received_signal(world, position) > 0;
    if powered == state.powered() {
        return;
    }
    set_powered(world, position, state, powered, "pulse_receiver_power");
    if powered {
        world.enqueue_block_event(BlockEvent {
            position,
            block: state.kind,
            action: 1,
            parameter: 0,
        });
    }
}

fn set_powered(world: &mut World, position: Position, state: BlockState, powered: bool, cause: &str) {
    let next = state.with_powered(powered).with_power(if powered { state.power().max(15) } else { 0 });
    if next != state {
        let _ = world.set_state(position, next, cause);
    }
}

pub(crate) fn trigger_button(world: &mut World, position: Position) {
    let state = world.state(position);
    if state.kind != BlockKind::Button || state.powered() {
        return;
    }
    set_powered(world, position, state, true, "button_use");
    notify_attached_conductors(world, position, BlockKind::Button);
    world.schedule_tick(position, BlockKind::Button, 20, TickPriority::Normal);
}

pub(crate) fn toggle_lever(world: &mut World, position: Position) {
    let state = world.state(position);
    if state.kind == BlockKind::Lever {
        set_powered(world, position, state, !state.powered(), "lever_use");
        notify_attached_conductors(world, position, BlockKind::Lever);
    }
}

pub(crate) fn set_external_power(world: &mut World, position: Position, powered: bool) -> Result<(), WorldError> {
    let state = world.state(position);
    let next = state
        .with_powered(powered)
        .with_power(if powered && state.kind == BlockKind::RedstoneWire { 15 } else { state.power() });
    world.set_state(position, next, "external_power")?;
    if matches!(state.kind, BlockKind::Lever | BlockKind::Button | BlockKind::PressurePlate) {
        notify_attached_conductors(world, position, state.kind);
    }
    if state.kind == BlockKind::Button && powered {
        world.schedule_tick(position, BlockKind::Button, 20, TickPriority::Normal);
    }
    Ok(())
}

pub(crate) fn set_external_signal(world: &mut World, position: Position, signal: u8) -> Result<(), WorldError> {
    let state = world.state(position);
    let signal = signal.min(15);
    let next = state.with_power(signal).with_powered(signal > 0);
    world.set_state(position, next, "external_signal")?;
    if state.kind == BlockKind::Target && signal > 0 {
        world.schedule_tick(position, BlockKind::Target, 20, TickPriority::Normal);
    }
    Ok(())
}

fn reset_target(world: &mut World, position: Position) {
    let state = world.state(position);
    if state.power() > 0 {
        let _ = world.set_state(position, state.with_power(0).with_powered(false), "target_timeout");
    }
}

fn notify_attached_conductors(world: &mut World, position: Position, changed_block: BlockKind) {
    for direction in Direction::ALL {
        let neighbor = position.offset(direction);
        if world.state(neighbor).kind.is_conductor() {
            world.update_neighbors_at(neighbor, changed_block);
        }
    }
}

pub(crate) fn queue_piston_event(world: &mut World, position: Position, action: u8, facing: Direction) {
    world.enqueue_block_event(BlockEvent {
        position,
        block: world.state(position).kind,
        action,
        parameter: facing as u8,
    });
}
