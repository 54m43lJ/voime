cmake_minimum_required(VERSION 3.20)
project(voime VERSION 0.1.0 LANGUAGES CXX)

include(GNUInstallDirs)
include(CTest)
include(cmake/install-addon.cmake)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(PkgConfig REQUIRED)
pkg_check_modules(FCITX5_CORE REQUIRED IMPORTED_TARGET Fcitx5Core)
pkg_get_variable(FCITX5_LIBDIR Fcitx5Core libdir)

add_library(voime MODULE
    src/config.cpp
    src/diagnostics.cpp
    src/ipc.cpp
    src/preview.cpp
    src/voime.cpp
)
target_include_directories(voime PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/third_party
)
target_link_libraries(voime PRIVATE PkgConfig::FCITX5_CORE)
target_compile_definitions(voime PRIVATE VOIME_FCITX5_LIBDIR="${FCITX5_LIBDIR}")
set_target_properties(voime PROPERTIES
    PREFIX ""
    OUTPUT_NAME "voime"
)

configure_file(voime-addon.conf.in voime.conf @ONLY)

voime_install_addon(voime)
install(FILES ${CMAKE_CURRENT_SOURCE_DIR}/../../assets/icons/voime.svg
        DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps
        RENAME voime.svg)

if(BUILD_TESTING)
    add_executable(voime-addon-tests
        tests/addon_test.cpp
        src/config.cpp
        src/ipc.cpp
    )
    target_include_directories(voime-addon-tests PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/src
        ${CMAKE_CURRENT_SOURCE_DIR}/third_party
    )
    target_link_libraries(voime-addon-tests PRIVATE PkgConfig::FCITX5_CORE)
    add_test(NAME voime-addon-tests COMMAND voime-addon-tests)
endif()
