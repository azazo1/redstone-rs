package redstone.oracle;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.lang.reflect.Constructor;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.PatchedDataComponentMap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.alchemy.PotionBrewing;
import net.minecraft.world.level.block.ComposterBlock;
import net.minecraft.world.level.block.entity.FuelValues;

final class ItemTraitsReport {
    private static final List<String> FUEL_TAGS = List.of(
        "minecraft:logs",
        "minecraft:bamboo_blocks",
        "minecraft:planks",
        "minecraft:wooden_stairs",
        "minecraft:wooden_slabs",
        "minecraft:wooden_trapdoors",
        "minecraft:wooden_pressure_plates",
        "minecraft:wooden_shelves",
        "minecraft:wooden_fences",
        "minecraft:fence_gates",
        "minecraft:banners",
        "minecraft:signs",
        "minecraft:hanging_signs",
        "minecraft:wooden_doors",
        "minecraft:boats",
        "minecraft:wool",
        "minecraft:wooden_buttons",
        "minecraft:saplings",
        "minecraft:wool_carpets"
    );

    private ItemTraitsReport() {
    }

    static void write(Path output, String version, int dataVersion) throws Exception {
        RegistryAccess.Frozen registries = RegistryAccess.fromRegistryOfRegistries(
            BuiltInRegistries.REGISTRY
        );
        FuelValues fuels = FuelValues.vanillaBurnTimes(
            registries,
            FeatureFlags.DEFAULT_FLAGS
        );
        Set<String> fuelItems = new HashSet<>();
        for (Item item : fuels.fuelItems()) {
            fuelItems.add(BuiltInRegistries.ITEM.getKey(item).toString());
        }
        TagResourceLoader itemTags = new TagResourceLoader("item");
        for (String tag : FUEL_TAGS) {
            fuelItems.addAll(itemTags.resolve(tag));
        }
        fuelItems.removeAll(itemTags.resolve("minecraft:non_flammable_wood"));
        PotionBrewing brewing = PotionBrewing.bootstrap(FeatureFlags.DEFAULT_FLAGS);
        List<Item> items = new ArrayList<>();
        BuiltInRegistries.ITEM.forEach(items::add);
        items.sort(Comparator.comparing(item -> BuiltInRegistries.ITEM.getKey(item).toString()));

        JsonObject root = new JsonObject();
        root.addProperty("version", version);
        root.addProperty("data_version", dataVersion);
        JsonArray entries = new JsonArray();
        for (Item item : items) {
            Identifier id = BuiltInRegistries.ITEM.getKey(item);
            ItemStack stack = stackWithoutDefaultComponents(item);
            JsonObject entry = new JsonObject();
            entry.addProperty("id", id.toString());
            entry.addProperty("furnace_fuel", fuelItems.contains(id.toString()));
            entry.addProperty("brewing_ingredient", brewing.isIngredient(stack));
            entry.addProperty(
                "compost_chance",
                ComposterBlock.COMPOSTABLES.getFloat(item)
            );
            entries.add(entry);
        }
        root.add("items", entries);

        Path normalized = output.toAbsolutePath().normalize();
        Path parent = normalized.getParent();
        if (parent != null) {
            Files.createDirectories(parent);
        }
        String json = new GsonBuilder()
            .disableHtmlEscaping()
            .setPrettyPrinting()
            .create()
            .toJson(root);
        Files.writeString(normalized, json + "\n");
        System.out.println("物品特征报告已生成: " + normalized);
        System.out.println("物品数量: " + items.size());
    }

    private static ItemStack stackWithoutDefaultComponents(Item item) throws Exception {
        Constructor<ItemStack> constructor = ItemStack.class.getDeclaredConstructor(
            net.minecraft.core.Holder.class,
            int.class,
            PatchedDataComponentMap.class
        );
        constructor.setAccessible(true);
        return constructor.newInstance(
            item.builtInRegistryHolder(),
            1,
            new PatchedDataComponentMap(DataComponentMap.EMPTY)
        );
    }

}
