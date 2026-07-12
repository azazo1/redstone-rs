package redstone.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

final class TagResourceLoader {
    private final String registry;
    private final Map<String, Set<String>> cache = new HashMap<>();
    private final Set<String> resolving = new HashSet<>();

    TagResourceLoader(String registry) {
        this.registry = registry;
    }

    Set<String> resolve(String id) throws Exception {
        return resolve(id, true);
    }

    private Set<String> resolve(String id, boolean required) throws Exception {
        String normalized = normalizeId(id);
        Set<String> cached = cache.get(normalized);
        if (cached != null) {
            return cached;
        }
        if (!resolving.add(normalized)) {
            throw new IllegalStateException("标签存在循环引用: " + normalized);
        }
        try {
            Set<String> values = load(normalized, required);
            Set<String> immutable = Set.copyOf(values);
            cache.put(normalized, immutable);
            return immutable;
        } finally {
            resolving.remove(normalized);
        }
    }

    private Set<String> load(String id, boolean required) throws Exception {
        int separator = id.indexOf(':');
        String namespace = id.substring(0, separator);
        String path = id.substring(separator + 1);
        String resource = "data/" + namespace + "/tags/" + registry + "/" + path + ".json";
        InputStream stream = TagResourceLoader.class.getClassLoader().getResourceAsStream(resource);
        if (stream == null) {
            if (!required) {
                return Set.of();
            }
            throw new IllegalStateException("client JAR 缺少标签资源: " + resource);
        }
        try (stream; InputStreamReader reader = new InputStreamReader(stream, StandardCharsets.UTF_8)) {
            JsonArray entries = JsonParser.parseReader(reader)
                .getAsJsonObject()
                .getAsJsonArray("values");
            Set<String> values = new HashSet<>();
            for (JsonElement entry : entries) {
                JsonObject object = entry.isJsonObject() ? entry.getAsJsonObject() : null;
                String value = object == null
                    ? entry.getAsString()
                    : object.get("id").getAsString();
                boolean entryRequired = object == null
                    || !object.has("required")
                    || object.get("required").getAsBoolean();
                if (value.startsWith("#")) {
                    values.addAll(resolve(value.substring(1), entryRequired));
                } else {
                    values.add(normalizeId(value));
                }
            }
            return values;
        }
    }

    private static String normalizeId(String id) {
        return id.contains(":") ? id : "minecraft:" + id;
    }
}
