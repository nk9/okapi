import os
import random

# Create directory if it doesn't exist
if not os.path.exists("ocr_csv_corpus"):
    os.makedirs("ocr_csv_corpus")

# Lists of names and other data
first_names = [
    "James", "John", "Robert", "Michael", "William", "David", "Richard", "Joseph",
    "Thomas", "Charles", "Christopher", "Daniel", "Matthew", "Anthony", "Donald",
    "Mark", "Paul", "Steven", "Andrew", "Kenneth", "Mary", "Patricia", "Jennifer",
    "Linda", "Elizabeth", "Barbara", "Susan", "Jessica", "Miss Jessica", "Sarah", "Miss Karen", "Nancy",
    "Lisa", "Mrs. Margaret", "Margaret", "Betty", "Sandra", "Ashley", "Dorothy", "Kimberly", "Emily",
    "Donna", "Michelle", "Carol", "Amanda", "Melissa", "Deborah"
]

last_names = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
    "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson",
    "Thomas", "Taylor", "Moore", "Jackson", "Martin", "Lee", "Perez", "Thompson",
    "White", "Harris", "Sanchez", "Clark", "Ramirez", "Lewis", "Robinson", "Walker",
    "Young", "Allen", "King", "Wright", "Scott", "Torres", "Nguyen", "Hill", "Flores",
    "Green", "Adams", "Nelson", "Baker", "Hall", "Rivera"
]

titles = ["Rev.", "Dr.", ""]

agencies = [
    "Department of Agriculture", "Department of Commerce", "Department of Defense",
    "Department of Education", "Department of Energy", "Department of Health and Human Services",
    "Department of Homeland Security", "Department of Housing and Urban Development",
    "Department of the Interior", "Department of Justice", "Department of Labor",
    "Department of State", "Department of Transportation", "Department of the Treasury",
    "Department of Veterans Affairs", "Environmental Protection Agency",
    "Social Security Administration", "National Aeronautics and Space Administration"
]

cities = [
    "New York", "Los Angeles", "Chicago", "Houston", "Phoenix", "Philadelphia",
    "San Antonio", "San Diego", "Dallas", "San Jose", "Austin", "Jacksonville",
    "Fort Worth", "Columbus", "Charlotte", "San Francisco", "Indianapolis", "Seattle",
    "Denver", "Washington"
]

# Generate 30 files
for i in range(1, 31):
    with open(f"ocr_csv_corpus/ocr_page_{i:02}.txt", "w") as f:
        # Generate 100 lines for each file
        for _ in range(100):
            first_name = random.choice(first_names)
            last_name = random.choice(last_names)
#             title = random.choice(titles)
            agency = random.choice(agencies)
            city = random.choice(cities)
            # Generate a salary around $1000
            salary = round(random.uniform(800, 1200), 2)

            f.write(f"{last_name}, {first_name}, {agency} ${salary} {city}\n")

print("Successfully created 30 files in the 'ocr_csv_corpus' directory.")
