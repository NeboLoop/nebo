# GoBot ProGuard Rules

# Keep Gson serialization
-keepattributes Signature
-keepattributes *Annotation*
-keep class com.gobot.app.** { *; }

# OkHttp
-dontwarn okhttp3.**
-dontwarn okio.**
-keep class okhttp3.** { *; }

# Retrofit
-keep class retrofit2.** { *; }
-keepclassmembers,allowobfuscation class * {
    @com.google.gson.annotations.SerializedName <fields>;
}
