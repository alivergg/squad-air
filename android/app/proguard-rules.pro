-keep class uniffi.squad_core.** { *; }
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class app.squadair.mobile.core.SquadNativeBridge {
    native <methods>;
    public static <methods>;
}
-dontwarn java.awt.**
