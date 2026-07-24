cmake_minimum_required(VERSION 3.14)
project(runner LANGUAGES CXX)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME} WIN32
  "flutter_window.cpp"
  "image_attachment_picker.cpp"
  "main.cpp"
  "title_bar_overlay.cpp"
  "utils.cpp"
  "win32_window.cpp"
  "window_controls.cpp"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
  "Runner.rc"
  "runner.exe.manifest"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# AppWindowTitleBar provides the Windows title-bar overlay while preserving the
# system caption buttons. Keep the Windows App SDK component self-contained so
# unpackaged Flutter builds do not require a separately installed runtime.
set_property(TARGET ${BINARY_NAME} PROPERTY VS_PACKAGE_REFERENCES
  "Microsoft.WindowsAppSDK.InteractiveExperiences_1.8.260125001;Microsoft.Windows.CppWinRT_2.0.230706.1")
set_property(TARGET ${BINARY_NAME} PROPERTY
  VS_GLOBAL_WindowsPackageType "None")
set_property(TARGET ${BINARY_NAME} PROPERTY
  VS_GLOBAL_WindowsAppSDKSelfContained "true")
# CMake's native vcxproj generator leaves TargetFrameworkMoniker empty, while
# NuGet restores native packages as "native,Version=v0.0". Keep MSBuild's
# package asset resolution on the same target.
set_property(TARGET ${BINARY_NAME} PROPERTY
  VS_GLOBAL_TargetFrameworkMoniker "native,Version=v0.0")
set_property(TARGET ${BINARY_NAME} PROPERTY
  VS_GLOBAL_NuGetTargetMoniker "native,Version=v0.0")

# Add preprocessor definitions for the build version.
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION=\"${FLUTTER_VERSION}\"")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MAJOR=${FLUTTER_VERSION_MAJOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MINOR=${FLUTTER_VERSION_MINOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_PATCH=${FLUTTER_VERSION_PATCH}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_BUILD=${FLUTTER_VERSION_BUILD}")

# Disable Windows macros that collide with C++ standard library functions.
target_compile_definitions(${BINARY_NAME} PRIVATE "NOMINMAX")

# Add dependency libraries and include directories. Add any application-specific
# dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter flutter_wrapper_app)
target_link_libraries(${BINARY_NAME} PRIVATE "dwmapi.lib" "ole32.lib" "windowscodecs.lib")
target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# Run the Flutter tool portions of the build. This must not be removed.
add_dependencies(${BINARY_NAME} flutter_assemble)
