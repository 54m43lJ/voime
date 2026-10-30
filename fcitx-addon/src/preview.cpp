#include "preview.h"

#include <fcitx/inputcontext.h>
#include <fcitx/inputpanel.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

void Preview::show(fcitx::InputContext *ic, const std::string &partial) {
    if (!ic) {
        return;
    }
    ic->inputPanel().setPreedit(fcitx::Text(partial));
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
}

void Preview::clear(fcitx::InputContext *ic) {
    if (!ic) {
        return;
    }
    ic->inputPanel().setPreedit(fcitx::Text());
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
}
