package redstone.oracle;

import java.lang.instrument.Instrumentation;

public final class OracleAgent {
    private OracleAgent() {
    }

    public static void premain(String arguments, Instrumentation instrumentation) {
        instrumentation.addTransformer(new OracleTransformer());
    }
}
