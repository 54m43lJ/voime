cmake_minimum_required(VERSION 3.20)
project(voime VERSION 0.1.0 LANGUAGES CXX)

include(GNUInstallDirs)
include(CTest)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(PkgConfig REQUIRED)
pkg_check_modules(FCITX5_CORE REQUIRED IMPORTED_TARGET Fcitx5Core)

add_library(voime MODULE
    src/config.cpp
    src/ipc.cpp
    src/preview.cpp
    src/voime.cpp
)
target_include_directories(voime PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/third_party
)
target_link_libraries(voime PRIVATE PkgConfig::FCITX5_CORE)
set_target_properties(voime PROPERTIES
    PREFIX ""
    OUTPUT_NAME "voime"
)

configure_file(voime-addon.conf.in voime.conf @ONLY)

install(TARGETS voime LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}/fcitx5)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/voime.conf
        DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/addon)
install(FILES inputmethod/voime.conf
        DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/inputmethod)
install(FILES icons/voime.svg
        DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps)

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
