#include <stdio.h>
#include <string.h>
#include <unistd.h>

void win() {
    printf("You win!\n");
}

void vuln() {
    char buffer[64];
    printf("Input something: ");
    // Read more than 64 bytes to cause overflow
    read(0, buffer, 200); 
    printf("You said: %s\n", buffer);
}

int main() {
    setvbuf(stdout, NULL, _IONBF, 0); // Disable buffering
    vuln();
    return 0;
}

