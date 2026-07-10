package redstone.oracle;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import net.minecraft.gametest.framework.GameTestServer;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.levelgen.WorldOptions;

final class OracleServerOptions {
    private OracleServerOptions() {
    }

    static void apply(Scenario scenario) throws Exception {
        Class<?> server = GameTestServer.class;
        Field featuresField = server.getDeclaredField("ENABLED_FEATURES");
        featuresField.setAccessible(true);
        FeatureFlagSet features = (FeatureFlagSet)featuresField.get(null);
        if (scenario.mode.equals("experimental")) {
            features = features.join(FeatureFlagSet.of(FeatureFlags.REDSTONE_EXPERIMENTS));
        }
        replaceStaticFinal(featuresField, features);

        Field worldOptions = server.getDeclaredField("WORLD_OPTIONS");
        worldOptions.setAccessible(true);
        replaceStaticFinal(worldOptions, new WorldOptions(scenario.seed, false, false));
    }

    private static void replaceStaticFinal(Field field, Object value) throws Exception {
        Class<?> unsafeClass = Class.forName("sun.misc.Unsafe");
        Field instanceField = unsafeClass.getDeclaredField("theUnsafe");
        instanceField.setAccessible(true);
        Object unsafe = instanceField.get(null);
        Method fieldBase = unsafeClass.getMethod("staticFieldBase", Field.class);
        Method fieldOffset = unsafeClass.getMethod("staticFieldOffset", Field.class);
        Method putObject = unsafeClass.getMethod("putObject", Object.class, long.class, Object.class);
        Object base = fieldBase.invoke(unsafe, field);
        long offset = (long)fieldOffset.invoke(unsafe, field);
        putObject.invoke(unsafe, base, offset, value);
    }
}
