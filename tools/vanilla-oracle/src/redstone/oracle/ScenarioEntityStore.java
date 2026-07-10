package redstone.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import java.lang.reflect.Field;
import java.math.BigDecimal;
import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.Map;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.resources.Identifier;
import net.minecraft.world.Container;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.decoration.ItemFrame;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.item.PrimedTnt;
import net.minecraft.world.entity.vehicle.minecart.MinecartHopper;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.Vec3;

final class ScenarioEntityStore {
    private static final Field ITEM_AGE = integerField(ItemEntity.class, "age");
    private static final Field ITEM_PICKUP_DELAY = integerField(ItemEntity.class, "pickupDelay");

    private final Scenario scenario;
    private final FrameGeometry frame;
    private final Map<Long, TrackedEntity> byId = new LinkedHashMap<>();
    private final Map<Entity, TrackedEntity> byEntity = new IdentityHashMap<>();

    ScenarioEntityStore(Scenario scenario, FrameGeometry frame) {
        this.scenario = scenario;
        this.frame = frame;
    }

    void spawn(GameTestHelper helper, Scenario.Action action) {
        if (action.entityId() != null) {
            TrackedEntity existing = byId.get(action.entityId());
            if (existing != null && !existing.entity.isRemoved()) {
                throw new IllegalArgumentException("实体 id 已存在: " + action.entityId());
            }
        }
        EntityType<?> type = resolveType(action.entityKind());
        Entity entity = type.create(helper.getLevel(), EntitySpawnReason.STRUCTURE);
        if (entity == null) {
            throw new IllegalArgumentException("无法创建实体: " + action.entityKind());
        }
        double[] relative = scenario.source.toFrame(action.position(), frame);
        Vec3 absolute = helper.absoluteVec(new Vec3(relative[0], relative[1], relative[2]));
        entity.setPos(absolute);

        Map<String, JsonElement> fields = new LinkedHashMap<>();
        action.fields().forEach((name, value) -> fields.put(name, value.deepCopy()));
        TrackedEntity tracked = new TrackedEntity(action.entityKind(), entity, fields);
        configureEntity(tracked);
        if (!helper.getLevel().addFreshEntity(entity)) {
            throw new IllegalStateException("实体加入世界失败: " + action.entityKind());
        }
        if (action.entityId() != null) {
            byId.put(action.entityId(), tracked);
        }
        byEntity.put(entity, tracked);
    }

    void move(GameTestHelper helper, long id, double[] position) {
        TrackedEntity tracked = require(id);
        double[] relative = scenario.source.toFrame(position, frame);
        Vec3 absolute = helper.absoluteVec(new Vec3(relative[0], relative[1], relative[2]));
        tracked.entity.setPos(absolute);
    }

    void remove(long id) {
        TrackedEntity tracked = require(id);
        tracked.entity.discard();
        byId.remove(id);
        byEntity.remove(tracked.entity);
    }

    void setField(long id, String field, JsonElement value) {
        TrackedEntity tracked = require(id);
        tracked.fields.put(field, value.deepCopy());
        applyField(tracked, field);
    }

    JsonElement readField(long id, String field) {
        TrackedEntity tracked = byId.get(id);
        if (tracked == null || tracked.entity.isRemoved()) {
            return JsonNull.INSTANCE;
        }
        JsonElement live = liveField(tracked, field);
        JsonElement value = live != null ? live : tracked.fields.get(field);
        return probeValue(value);
    }

    int containerCount(long id) {
        TrackedEntity tracked = byId.get(id);
        if (tracked == null || tracked.entity.isRemoved() || !(tracked.entity instanceof Container container)) {
            return 0;
        }
        int count = 0;
        for (int slot = 0; slot < container.getContainerSize(); slot++) {
            count = Math.addExact(count, container.getItem(slot).getCount());
        }
        return count;
    }

    int entityCount(GameTestHelper helper, String expectedKind) {
        int count = 0;
        for (Entity entity : helper.getLevel().getAllEntities()) {
            if (entity.isRemoved()) {
                continue;
            }
            String kind = logicalKind(entity);
            if (expectedKind == null || expectedKind.equals(kind)) {
                count = Math.addExact(count, 1);
            }
        }
        return count;
    }

    private void configureEntity(TrackedEntity tracked) {
        if (tracked.entity instanceof ItemEntity item) {
            item.setItem(itemStack(tracked.fields, "minecraft:stone", 1));
        }
        if (tracked.entity instanceof Container container) {
            configureContainer(container, tracked.fields);
        }
        for (String field : tracked.fields.keySet()) {
            applyField(tracked, field);
        }
    }

    private void applyField(TrackedEntity tracked, String field) {
        JsonElement value = tracked.fields.get(field);
        switch (field) {
            case "no_gravity" -> tracked.entity.setNoGravity(booleanValue(value, field));
            case "age" -> {
                if (tracked.entity instanceof ItemEntity item) {
                    setInteger(ITEM_AGE, item, integerValue(value, field));
                }
            }
            case "pickup_delay" -> {
                if (tracked.entity instanceof ItemEntity item) {
                    item.setPickUpDelay(integerValue(value, field));
                }
            }
            case "item_id", "item_count" -> {
                if (tracked.entity instanceof ItemEntity item) {
                    item.setItem(itemStack(tracked.fields, "minecraft:stone", 1));
                } else if (tracked.entity instanceof Container container
                    && !tracked.fields.containsKey("inventory")) {
                    container.setItem(0, itemStack(tracked.fields, "minecraft:stone", 0));
                    container.setChanged();
                } else if (tracked.entity instanceof ItemFrame itemFrame) {
                    itemFrame.setItem(itemStack(tracked.fields, "minecraft:air", 0), false);
                }
            }
            case "inventory" -> {
                if (tracked.entity instanceof Container container) {
                    configureContainer(container, tracked.fields);
                }
            }
            case "enabled" -> {
                if (tracked.entity instanceof MinecartHopper hopper) {
                    hopper.setEnabled(booleanValue(value, field));
                }
            }
            case "fuse" -> {
                if (tracked.entity instanceof PrimedTnt tnt) {
                    tnt.setFuse(integerValue(value, field));
                }
            }
            case "rotation" -> {
                if (tracked.entity instanceof ItemFrame itemFrame) {
                    itemFrame.setRotation(integerValue(value, field));
                }
            }
            default -> {
            }
        }
    }

    private JsonElement liveField(TrackedEntity tracked, String field) {
        if (tracked.entity instanceof ItemEntity item) {
            return switch (field) {
                case "item_id" -> new JsonPrimitive(itemId(item.getItem()));
                case "item_count" -> new JsonPrimitive(item.getItem().getCount());
                case "age" -> new JsonPrimitive(item.getAge());
                case "pickup_delay" -> new JsonPrimitive(getInteger(ITEM_PICKUP_DELAY, item));
                default -> null;
            };
        }
        if (tracked.entity instanceof MinecartHopper hopper && field.equals("enabled")) {
            return new JsonPrimitive(hopper.isEnabled());
        }
        if (tracked.entity instanceof PrimedTnt tnt && field.equals("fuse")) {
            return new JsonPrimitive(tnt.getFuse());
        }
        if (tracked.entity instanceof ItemFrame itemFrame) {
            return switch (field) {
                case "item_id" -> new JsonPrimitive(itemId(itemFrame.getItem()));
                case "item_count" -> new JsonPrimitive(itemFrame.getItem().getCount());
                case "rotation" -> new JsonPrimitive(itemFrame.getRotation());
                default -> null;
            };
        }
        if (tracked.entity instanceof Container container && field.equals("inventory")) {
            return inventory(container);
        }
        if (field.equals("no_gravity")) {
            return new JsonPrimitive(tracked.entity.isNoGravity());
        }
        return null;
    }

    static void configureContainer(Container container, Map<String, JsonElement> fields) {
        JsonElement inventory = fields.get("inventory");
        if (inventory == null) {
            if (fields.containsKey("item_id") || fields.containsKey("item_count")) {
                container.setItem(0, itemStack(fields, "minecraft:stone", 0));
                container.setChanged();
            }
            return;
        }
        if (!inventory.isJsonArray()) {
            throw new IllegalArgumentException("inventory 必须是数组");
        }
        container.clearContent();
        for (JsonElement entryValue : inventory.getAsJsonArray()) {
            if (!entryValue.isJsonObject()) {
                throw new IllegalArgumentException("inventory 条目必须是对象");
            }
            JsonObject entry = entryValue.getAsJsonObject();
            int slot = integerValue(required(entry, "slot"), "inventory.slot");
            if (slot < 0 || slot >= container.getContainerSize()) {
                throw new IllegalArgumentException("inventory slot 越界: " + slot);
            }
            Map<String, JsonElement> item = Map.of(
                "item_id",
                required(entry, "item_id"),
                "item_count",
                required(entry, "count")
            );
            container.setItem(slot, itemStack(item, "minecraft:air", 0));
        }
        container.setChanged();
    }

    private static JsonArray inventory(Container container) {
        JsonArray result = new JsonArray();
        for (int slot = 0; slot < container.getContainerSize(); slot++) {
            ItemStack stack = container.getItem(slot);
            if (stack.isEmpty()) {
                continue;
            }
            JsonObject entry = new JsonObject();
            entry.addProperty("slot", slot);
            entry.addProperty("item_id", itemId(stack));
            entry.addProperty("count", stack.getCount());
            result.add(entry);
        }
        return result;
    }

    private String logicalKind(Entity entity) {
        TrackedEntity tracked = byEntity.get(entity);
        if (tracked != null) {
            return tracked.kind;
        }
        Identifier id = BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType());
        return id == null ? "minecraft:unknown" : id.toString();
    }

    private static EntityType<?> resolveType(String kind) {
        if (kind.equals("minecraft:generic_collision")) {
            return EntityType.ARMOR_STAND;
        }
        Identifier id = Identifier.parse(kind);
        if (!BuiltInRegistries.ENTITY_TYPE.containsKey(id)) {
            throw new IllegalArgumentException("未知实体: " + kind);
        }
        return BuiltInRegistries.ENTITY_TYPE.getValue(id);
    }

    private TrackedEntity require(long id) {
        TrackedEntity tracked = byId.get(id);
        if (tracked == null || tracked.entity.isRemoved()) {
            throw new IllegalArgumentException("实体不存在: " + id);
        }
        return tracked;
    }

    private static ItemStack itemStack(
        Map<String, JsonElement> fields,
        String defaultItem,
        int defaultCount
    ) {
        String itemId = fields.containsKey("item_id")
            ? stringValue(fields.get("item_id"), "item_id")
            : defaultItem;
        int count = fields.containsKey("item_count")
            ? integerValue(fields.get("item_count"), "item_count")
            : defaultCount;
        if (count < 0) {
            throw new IllegalArgumentException("item_count 不能是负数: " + count);
        }
        Identifier id = Identifier.parse(itemId);
        if (!BuiltInRegistries.ITEM.containsKey(id)) {
            throw new IllegalArgumentException("未知物品: " + itemId);
        }
        Item item = BuiltInRegistries.ITEM.getValue(id);
        return new ItemStack(item, count);
    }

    private static String itemId(ItemStack stack) {
        Identifier id = BuiltInRegistries.ITEM.getKey(stack.getItem());
        return id == null ? "minecraft:air" : id.toString();
    }

    private static JsonElement required(JsonObject object, String field) {
        JsonElement value = object.get(field);
        if (value == null) {
            throw new IllegalArgumentException("缺少实体字段: " + field);
        }
        return value;
    }

    private static String stringValue(JsonElement value, String field) {
        if (value == null || !value.isJsonPrimitive() || !value.getAsJsonPrimitive().isString()) {
            throw new IllegalArgumentException("实体字段必须是字符串: " + field);
        }
        return value.getAsString();
    }

    private static boolean booleanValue(JsonElement value, String field) {
        if (value == null || !value.isJsonPrimitive() || !value.getAsJsonPrimitive().isBoolean()) {
            throw new IllegalArgumentException("实体字段必须是布尔值: " + field);
        }
        return value.getAsBoolean();
    }

    private static int integerValue(JsonElement value, String field) {
        if (value == null || !value.isJsonPrimitive() || !value.getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException("实体字段必须是整数: " + field);
        }
        try {
            return new BigDecimal(value.getAsString()).intValueExact();
        } catch (ArithmeticException error) {
            throw new IllegalArgumentException("实体字段超出整数范围: " + field, error);
        }
    }

    private static JsonElement probeValue(JsonElement value) {
        if (value == null || value.isJsonNull()) {
            return JsonNull.INSTANCE;
        }
        if (value.isJsonArray() || value.isJsonObject()) {
            return new JsonPrimitive(value.toString());
        }
        JsonPrimitive primitive = value.getAsJsonPrimitive();
        if (!primitive.isNumber()) {
            return primitive.deepCopy();
        }
        try {
            return new JsonPrimitive(new BigDecimal(primitive.getAsString()).longValueExact());
        } catch (ArithmeticException error) {
            return JsonNull.INSTANCE;
        }
    }

    private static Field integerField(Class<?> owner, String name) {
        try {
            Field field = owner.getDeclaredField(name);
            field.setAccessible(true);
            return field;
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static int getInteger(Field field, Object owner) {
        try {
            return field.getInt(owner);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("无法读取实体整数字段", error);
        }
    }

    private static void setInteger(Field field, Object owner, int value) {
        try {
            field.setInt(owner, value);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("无法写入实体整数字段", error);
        }
    }

    private static final class TrackedEntity {
        final String kind;
        final Entity entity;
        final Map<String, JsonElement> fields;

        TrackedEntity(String kind, Entity entity, Map<String, JsonElement> fields) {
            this.kind = kind;
            this.entity = entity;
            this.fields = fields;
        }
    }
}
