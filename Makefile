main: main.c
	gcc -o main -lmpdclient -lsqlite3 -Wall -Werror main.c