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
const GEAR_INTERVAL: Duration = Duration::from_millis(400);
const HUNGRY_WARN_INTERVAL: Duration = Duration::from_secs(15);
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
    last_gear_swap: Instant,
    last_hungry_warn: Instant,
}

impl Default for Combat {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            eat: EatState::Idle,
            last_eat_check: now,
            last_totem_swap: now,
            last_gear_swap: now,
            last_hungry_warn: now.checked_sub(HUNGRY_WARN_INTERVAL).unwrap_or(now),
        }
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
    if settings.auto_armor && matches!(state.eat, EatState::Idle) {
        armor(bot, &mut state, &menu, &slots);
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
            let previous = bot.selected_hotbar_slot();
            let mut food_slot = hotbar.clone().find(|i| slots.get(*i).is_some_and(is_good_food));
            if food_slot.is_none() {
                let in_inventory = (9..first).find(|i| slots.get(*i).is_some_and(is_good_food));
                if let Some(source) = in_inventory {
                    bot.get_inventory().click(SwapClick { source_slot: source as u16, target_slot: previous });
                    food_slot = Some(first + previous as usize);
                }
            }
            let Some(food_slot) = food_slot else {
                if state.last_hungry_warn.elapsed() >= HUNGRY_WARN_INTERVAL {
                    state.last_hungry_warn = now;
                    warn!("Hungry (food level {}) but no usable food in the inventory.", hunger.food);
                }
                return;
            };
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
    if settings.auto_weapon && equip_weapon(bot) {
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

fn armor_tier(name: &str) -> i32 {
    if name.starts_with("Netherite") {
        6
    } else if name.starts_with("Diamond") {
        5
    } else if name.starts_with("Iron") {
        4
    } else if name.starts_with("Chainmail") {
        3
    } else if name.starts_with("Golden") || name.starts_with("Turtle") {
        2
    } else if name.starts_with("Leather") {
        1
    } else {
        0
    }
}

fn armor_info(stack: &ItemStack) -> Option<(usize, i32)> {
    if !stack.is_present() {
        return None;
    }
    let name = item_name(stack);
    let slot = if name.ends_with("Helmet") {
        5
    } else if name.ends_with("Chestplate") {
        6
    } else if name.ends_with("Leggings") {
        7
    } else if name.ends_with("Boots") {
        8
    } else {
        return None;
    };
    Some((slot, armor_tier(&name)))
}

fn worn_score(stack: &ItemStack) -> i32 {
    if !stack.is_present() {
        return 0;
    }
    if item_name(stack) == "Elytra" {
        return 100;
    }
    armor_info(stack).map_or(0, |(_, score)| score)
}

fn armor(bot: &Client, state: &mut Combat, menu: &Menu, slots: &[ItemStack]) {
    if state.last_gear_swap.elapsed() < GEAR_INTERVAL {
        return;
    }
    let hotbar = menu.hotbar_slots_range();
    let first = *hotbar.start();
    let selected = bot.selected_hotbar_slot();
    for armor_slot in 5..=8usize {
        let current = slots.get(armor_slot).map_or(0, worn_score);
        let mut best: Option<(usize, i32)> = None;
        for i in 9..first + 9 {
            let Some(stack) = slots.get(i) else {
                continue;
            };
            if let Some((slot, score)) = armor_info(stack)
                && slot == armor_slot
                && score > current
                && best.is_none_or(|b| score > b.1)
            {
                best = Some((i, score));
            }
        }
        let Some((source, _)) = best else {
            continue;
        };
        state.last_gear_swap = Instant::now();
        if source >= first {
            bot.get_inventory().click(SwapClick { source_slot: armor_slot as u16, target_slot: (source - first) as u8 });
            info!("Equipped better armor into slot {armor_slot}");
        } else {
            bot.get_inventory().click(SwapClick { source_slot: source as u16, target_slot: selected });
            info!("Moving armor from inventory slot {source} to the hotbar to equip it");
        }
        return;
    }
}

fn weapon_score(stack: &ItemStack) -> i32 {
    if !stack.is_present() {
        return 0;
    }
    let name = item_name(stack);
    let tier = if name.starts_with("Netherite") {
        6
    } else if name.starts_with("Diamond") {
        5
    } else if name.starts_with("Iron") {
        4
    } else if name.starts_with("Stone") {
        3
    } else if name.starts_with("Wooden") {
        2
    } else if name.starts_with("Golden") {
        1
    } else {
        return 0;
    };
    if name.ends_with("Sword") {
        tier * 10 + 5
    } else if name.ends_with("Axe") {
        tier * 10
    } else {
        0
    }
}

fn equip_weapon(bot: &Client) -> bool {
    let menu = bot.menu();
    if !matches!(menu, Menu::Player(_)) {
        return false;
    }
    let slots = menu.slots();
    let hotbar = menu.hotbar_slots_range();
    let first = *hotbar.start();
    let selected = bot.selected_hotbar_slot();
    let held = slots.get(first + selected as usize).map_or(0, weapon_score);
    let best_hotbar = hotbar
        .filter_map(|i| slots.get(i).map(|s| (i, weapon_score(s))))
        .filter(|(_, score)| *score > held)
        .max_by_key(|(_, score)| *score);
    let best_inventory = (9..first)
        .filter_map(|i| slots.get(i).map(|s| (i, weapon_score(s))))
        .filter(|(_, score)| *score > held)
        .max_by_key(|(_, score)| *score);
    match (best_hotbar, best_inventory) {
        (Some((i, hs)), inv) if inv.is_none_or(|(_, is)| hs >= is) => {
            bot.set_selected_hotbar_slot((i - first) as u8);
            true
        }
        (_, Some((i, _))) => {
            bot.get_inventory().click(SwapClick { source_slot: i as u16, target_slot: selected });
            true
        }
        _ => false,
    }
}
