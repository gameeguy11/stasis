use crate::cfg;
use crate::config::normalize_name;
use azalea::ecs::query::{With, Without};
use azalea::entity::metadata::{AbstractMonster, Enderman, Health, Piglin, Player, ZombifiedPiglin};
use azalea::entity::{Dead, EntityKindComponent, EntityUuid, LocalEntity, Position};
use azalea::inventory::components::Food;
use azalea::inventory::operations::SwapClick;
use azalea::inventory::{ItemStack, Menu};
use azalea::local_player::Hunger;
use azalea::prelude::*;
use azalea::{Client, Vec3};
use parking_lot::Mutex;
use std::time::{Duration, Instant};

const OFFHAND_MENU_SLOT: usize = 45;
const OFFHAND_SWAP_TARGET: u8 = 40;
const SWITCH_DELAY: Duration = Duration::from_millis(150);
const EAT_DURATION: Duration = Duration::from_millis(1900);
const EAT_CHECK_INTERVAL: Duration = Duration::from_millis(500);
const TOTEM_SWAP_INTERVAL: Duration = Duration::from_millis(250);
const JUNK_FOOD: [&str; 8] = [
    "ChorusFruit",
    "PoisonousPotato",
    "SpiderEye",
    "Pufferfish",
    "RottenFlesh",
    "GoldenApple",
    "EnchantedGoldenApple",
    "SuspiciousStew",
];

#[derive(Clone, Copy)]
enum EatState {
    Idle,
    Switching { at: Instant, previous: u8 },
    Eating { until: Instant, previous: u8 },
}

pub struct Combat {
    eat: EatState,
    last_eat_check: Instant,
    last_totem_swap: Instant,
}

impl Default for Combat {
    fn default() -> Self {
        let now = Instant::now();
        Self { eat: EatState::Idle, last_eat_check: now, last_totem_swap: now }
    }
}

fn item_name(stack: &ItemStack) -> String {
    format!("{:?}", stack.kind())
}

fn is_totem(stack: &ItemStack) -> bool {
    stack.is_present() && item_name(stack) == "TotemOfUndying"
}

fn is_good_food(stack: &ItemStack) -> bool {
    if !stack.is_present() || stack.get_component::<Food>().is_none() {
        return false;
    }
    let name = item_name(stack);
    !JUNK_FOOD.contains(&name.as_str())
}

pub fn tick(bot: &Client, combat: &Mutex<Combat>) {
    match bot.get_component::<Health>() {
        Some(health) if *health > 0.0 => {}
        _ => return,
    }
    let settings = cfg();
    let menu = bot.menu();
    if !matches!(menu, Menu::Player(_)) {
        return;
    }
    let slots = menu.slots();
    let mut state = combat.lock();
    if settings.auto_totem {
        totem(bot, &mut state, &menu, &slots);
    }
    if settings.auto_eat {
        eat(bot, &mut state, &menu, &slots);
    }
    if settings.kill_aura && matches!(state.eat, EatState::Idle) {
        kill_aura(bot, settings);
    }
}

fn totem(bot: &Client, state: &mut Combat, menu: &Menu, slots: &[ItemStack]) {
    if slots.get(OFFHAND_MENU_SLOT).is_some_and(is_totem) {
        return;
    }
    if state.last_totem_swap.elapsed() < TOTEM_SWAP_INTERVAL {
        return;
    }
    let source = menu.player_slots_range().find(|i| slots.get(*i).is_some_and(is_totem));
    if let Some(source) = source {
        state.last_totem_swap = Instant::now();
        bot.get_inventory().click(SwapClick { source_slot: source as u16, target_slot: OFFHAND_SWAP_TARGET });
        info!("Moved a totem into the offhand (slot {source})");
    }
}

fn eat(bot: &Client, state: &mut Combat, menu: &Menu, slots: &[ItemStack]) {
    let now = Instant::now();
    match state.eat {
        EatState::Idle => {
            if state.last_eat_check.elapsed() < EAT_CHECK_INTERVAL {
                return;
            }
            state.last_eat_check = now;
            let Some(hunger) = bot.get_component::<Hunger>() else {
                return;
            };
            if hunger.food > cfg().eat_below {
                return;
            }
            let hotbar = menu.hotbar_slots_range();
            let first = *hotbar.start();
            let food_slot = hotbar.into_iter().find(|i| slots.get(*i).is_some_and(is_good_food));
            let Some(food_slot) = food_slot else {
                return;
            };
            let previous = bot.selected_hotbar_slot();
            bot.set_selected_hotbar_slot((food_slot - first) as u8);
            info!("Eating (food level {}, slot {})", hunger.food, food_slot - first);
            state.eat = EatState::Switching { at: now, previous };
        }
        EatState::Switching { at, previous } => {
            if at.elapsed() >= SWITCH_DELAY {
                bot.start_use_item();
                state.eat = EatState::Eating { until: now + EAT_DURATION, previous };
            }
        }
        EatState::Eating { until, previous } => {
            if now >= until {
                bot.set_selected_hotbar_slot(previous);
                state.eat = EatState::Idle;
                state.last_eat_check = now;
            }
        }
    }
}

fn listed(list: &[String], name: &str) -> bool {
    let name = normalize_name(name);
    !name.is_empty() && list.iter().any(|n| normalize_name(n) == name)
}

fn kill_aura(bot: &Client, settings: &crate::config::Config) {
    if bot.has_attack_cooldown() {
        return;
    }
    let range = settings.kill_aura_range;
    let targets = bot.nearest_entities_by::<(), (Without<LocalEntity>, Without<Dead>)>(|_: ()| true);
    let eye = bot.eye_position();
    let tab_list = bot.tab_list();
    let players = bot.nearest_entities_by::<(), (With<Player>, Without<LocalEntity>)>(|_: ()| true);
    let monsters = bot.nearest_entities_by::<(), (With<AbstractMonster>, Without<LocalEntity>)>(|_: ()| true);
    let endermen = bot.nearest_entities_by::<(), With<Enderman>>(|_: ()| true);
    let zombified = bot.nearest_entities_by::<(), With<ZombifiedPiglin>>(|_: ()| true);
    let piglins = bot.nearest_entities_by::<(), With<Piglin>>(|_: ()| true);
    for entity in targets {
        let Some(position) = bot.get_entity_component::<Position>(entity) else {
            continue;
        };
        let feet = Vec3::from(&position);
        let center = Vec3::new(feet.x, feet.y + 0.9, feet.z);
        let (dx, dy, dz) = (center.x - eye.x, center.y - eye.y, center.z - eye.z);
        if dx * dx + dy * dy + dz * dz > range * range {
            continue;
        }
        let Some(kind) = bot.get_entity_component::<EntityKindComponent>(entity) else {
            continue;
        };
        let kind_name = format!("{:?}", kind.0);
        if listed(&settings.kill_aura_never, &kind_name) {
            continue;
        }
        let allowed = if players.contains(&entity) {
            let name = bot
                .get_entity_component::<EntityUuid>(entity)
                .and_then(|uuid| tab_list.get(&uuid).map(|p| p.profile.name.clone()))
                .unwrap_or_default();
            settings.kill_aura_players && !listed(&settings.kill_aura_never, &name)
        } else if listed(&settings.kill_aura_always, &kind_name) {
            true
        } else if monsters.contains(&entity) {
            let neutral = endermen.contains(&entity) || zombified.contains(&entity) || piglins.contains(&entity);
            if neutral { settings.kill_aura_neutral } else { settings.kill_aura_hostiles }
        } else {
            false
        };
        if !allowed {
            continue;
        }
        bot.look_at(center);
        bot.attack(entity);
        return;
    }
}
