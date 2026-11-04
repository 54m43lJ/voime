#pragma once

#include <filesystem>
#include <string>

struct AddonPaths {
    std::filesystem::path library;
    std::filesystem::path description;
};

AddonPaths systemAddonPaths();
bool checkAddonAbi(std::string *error);
