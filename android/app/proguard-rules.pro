# JNI entry points are resolved by name.
-keepclasseswithmembernames class * {
    native <methods>;
}
-keep class org.harmion.android.core.HarmionNative { *; }
-keep class org.harmion.sdk.core.HarmionNative { *; }
-keep class org.harmion.android.service.IHarmionNodeService { *; }
-keep class org.harmion.android.service.IHarmionNodeService$Stub { *; }
-keep class org.harmion.android.service.IHarmionNodeService$Stub$Proxy { *; }
# App model JSON is parsed reflectively by org.json only; no keep needed.
