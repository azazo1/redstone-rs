package redstone.oracle;

import java.nio.file.Path;
import java.util.function.BiConsumer;
import java.util.function.Consumer;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.registries.Registries;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.gametest.framework.TestFunctionLoader;

final class ScenarioTestLoader extends TestFunctionLoader {
    static final String SCENARIO_ENV = "REDSTONE_ORACLE_SCENARIO";
    static final String OUTPUT_ENV = "REDSTONE_ORACLE_OUTPUT";

    private final Scenario scenario;
    private final Path output;

    ScenarioTestLoader(Scenario scenario, Path output) {
        this.scenario = scenario;
        this.output = output;
    }

    static ScenarioTestLoader fromEnvironment() throws Exception {
        String scenarioPath = System.getenv(SCENARIO_ENV);
        String outputPath = System.getenv(OUTPUT_ENV);
        if (scenarioPath == null || outputPath == null) {
            throw new IllegalStateException("GameTest 子进程缺少 oracle 环境变量");
        }
        Scenario scenario = Scenario.load(Path.of(scenarioPath));
        scenario.validateSupported();
        return new ScenarioTestLoader(scenario, Path.of(outputPath).toAbsolutePath().normalize());
    }

    @Override
    public void load(BiConsumer<ResourceKey<Consumer<GameTestHelper>>, Consumer<GameTestHelper>> register) {
        ResourceKey<Consumer<GameTestHelper>> key = ResourceKey.create(
            Registries.TEST_FUNCTION,
            Identifier.parse("redstone:scenario")
        );
        register.accept(key, helper -> new ScenarioTest(scenario, output).run(helper));
    }
}
