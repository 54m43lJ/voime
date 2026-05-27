cmake_minimum_required(VERSION 3.20)
project(voime-ime LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_definitions(-DUNICODE -D_UNICODE -DWIN32_LEAN_AND_MEAN -DNOMINMAX)

add_library(voime-ime SHARED
    src/main.cpp
    src/ime_service.cpp
    src/audio_capture.cpp
    src/ws_client.cpp
    src/registry.cpp
)

target_include_directories(voime-ime PRIVATE src)

target_link_libraries(voime-ime PRIVATE
    advapi32
    ole32
    oleaut32
    user32
    shell32
    winhttp
)

set_target_properties(voime-ime PROPERTIES
    OUTPUT_NAME "voime-ime"
    PREFIX ""
)

if(MSVC)
    set_target_properties(voime-ime PROPERTIES
        LINK_FLAGS "/EXPORT:DllGetClassObject /EXPORT:DllCanUnloadNow /EXPORT:DllRegisterServer /EXPORT:DllUnregisterServer"
    )
endif()
